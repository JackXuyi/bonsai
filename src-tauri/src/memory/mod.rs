//! L2 — Memory layer: local SQLite, local-first, explainable, forgettable.
//!
//! Research constraints (report §4.2) and plan (docs/zh/plan-memory.md):
//! - Explainable: every derived fact traces back to source events.
//! - Forgettable: "forget this" truly deletes (compliance Art. 16).
//! - Local-first: relationship data never leaves the machine by default.
//! - Hybrid retrieval: keyword + time decay (vector leg is a follow-up).
//!
//! Concurrency (docs/zh/plan-store-lock.md)：
//! - 1× writer `Mutex<Connection>`（写权威）
//! - N× reader（默认 2，`query_only`）吃 WAL 多读红利
//! - 长 IO（LLM / MCP / sleep）**不得**持任何 guard（2026-11-20 lock-narrowing）

pub mod clock;
pub mod consolidate;
pub mod db;
pub mod evidence;
pub mod forget;
pub mod implicit;
pub mod job;
/// S0 检索基线（2026-11-23）：只读量尺，进 release 包没有任何意义，
/// 因此只在测试构建里编译。
#[cfg(test)]
pub mod eval_recall;
pub mod eval;
pub mod extract;
pub mod index;
pub mod overview;
pub mod profile;
pub mod scheduler;
pub mod synonym;

use std::ops::Deref;
use std::sync::{Mutex, MutexGuard};

use rusqlite::Connection;
use serde::Serialize;

/// plan-store-lock D2：两条只读连接——对话装配与 UI/后台 tick 互不堵。
pub const READER_COUNT: usize = 2;

fn poison_err() -> rusqlite::Error {
    // plan-retry §5.6 / plan-store-lock：中毒不做 unsafe 恢复；前缀让前端可辨。
    rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error::new(1),
        Some(
            "internal_poisoned: memory store mutex poisoned — 内部状态已损坏，请重启应用".into(),
        ),
    )
}

/// 只读连接池：每条连接一把 Mutex；`try_lock` 轮询，全忙则阻塞第一条。
struct ReadPool {
    conns: Vec<Mutex<Connection>>,
}

impl ReadPool {
    fn open(path: &std::path::Path, n: usize) -> rusqlite::Result<Self> {
        let mut conns = Vec::with_capacity(n);
        for _ in 0..n {
            conns.push(Mutex::new(db::open_reader(path)?));
        }
        Ok(ReadPool { conns })
    }

    fn empty() -> Self {
        ReadPool { conns: Vec::new() }
    }

    fn lock(&self) -> rusqlite::Result<MutexGuard<'_, Connection>> {
        for m in &self.conns {
            if let Ok(g) = m.try_lock() {
                return Ok(g);
            }
        }
        self.conns
            .first()
            .ok_or_else(|| {
                rusqlite::Error::SqliteFailure(
                    rusqlite::ffi::Error::new(1),
                    Some("internal: read pool empty".into()),
                )
            })?
            .lock()
            .map_err(|_| poison_err())
    }
}

/// 进程内记忆库入口（plan-store-lock）：写走 `write`，只读走 `read`。
pub struct MemoryStore {
    write: Mutex<Connection>,
    read: ReadPool,
}

/// 只读守卫：deref → `&Connection`；drop 即还池。
pub struct ReadGuard<'a> {
    guard: MutexGuard<'a, Connection>,
}

impl Deref for ReadGuard<'_> {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        &self.guard
    }
}

impl MemoryStore {
    pub fn open(path: &std::path::Path) -> rusqlite::Result<Self> {
        // writer 先开并 migrate；再开 readers（不再跑 migrate）。
        let write = db::open(path)?;
        // Phase 3 A5：种子常驻 job（幂等；与 schema migrate 解耦避免循环依赖）。
        job::seed_builtin_jobs(&write)?;
        let read = ReadPool::open(path, READER_COUNT)?;
        Ok(MemoryStore {
            write: Mutex::new(write),
            read,
        })
    }

    /// 单连接测试夹具：无读池，`with_read` 降级到 write（同锁）。
    pub fn from_connection(conn: Connection) -> Self {
        MemoryStore {
            write: Mutex::new(conn),
            read: ReadPool::empty(),
        }
    }

    /// 写锁守卫。复杂多语句块用这个；能收成闭包的优先 [`with_write`]。
    pub fn write_guard(&self) -> rusqlite::Result<MutexGuard<'_, Connection>> {
        self.write.lock().map_err(|_| poison_err())
    }

    /// 非阻塞写锁（测试：断言推理期间零持锁）。毒锁同样映射为 `internal_poisoned`。
    pub fn try_write_guard(&self) -> rusqlite::Result<Option<MutexGuard<'_, Connection>>> {
        match self.write.try_lock() {
            Ok(g) => Ok(Some(g)),
            Err(std::sync::TryLockError::WouldBlock) => Ok(None),
            Err(std::sync::TryLockError::Poisoned(_)) => Err(poison_err()),
        }
    }

    /// 读锁守卫。读池空（测试夹具）时降级为写锁，语义仍安全、只是不并行。
    pub fn read_guard(&self) -> rusqlite::Result<ReadGuard<'_>> {
        if self.read.conns.is_empty() {
            return Ok(ReadGuard {
                guard: self.write_guard()?,
            });
        }
        Ok(ReadGuard {
            guard: self.read.lock()?,
        })
    }

    pub fn with_write<T>(
        &self,
        f: impl FnOnce(&Connection) -> rusqlite::Result<T>,
    ) -> rusqlite::Result<T> {
        let guard = self.write_guard()?;
        f(&*guard)
    }

    pub fn with_write_mut<T>(
        &self,
        f: impl FnOnce(&mut Connection) -> rusqlite::Result<T>,
    ) -> rusqlite::Result<T> {
        let mut guard = self.write_guard()?;
        f(&mut *guard)
    }

    pub fn with_read<T>(
        &self,
        f: impl FnOnce(&Connection) -> rusqlite::Result<T>,
    ) -> rusqlite::Result<T> {
        let guard = self.read_guard()?;
        f(&*guard)
    }
}

#[derive(Debug, Serialize)]
pub struct MemoryHit {
    pub kind: String,
    pub id: i64,
    pub ts: String,
    pub content: String,
    pub invalidated: bool,
}

#[tauri::command]
pub fn add_event(
    state: tauri::State<'_, MemoryStore>,
    kind: String,
    content: String,
    role: Option<String>,
    session_id: Option<String>,
) -> Result<i64, String> {
    state
        .inner()
        .with_write(|conn| {
            db::add_event_meta(&conn, &kind, &content, role.as_deref(), session_id.as_deref())
        })
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn add_fact(
    state: tauri::State<'_, MemoryStore>,
    content: String,
    source_event_id: Option<i64>,
) -> Result<i64, String> {
    state
        .inner()
        .with_write(|conn| db::add_fact(conn, &content, source_event_id))
        .map_err(|e| e.to_string())
}

/// Hybrid retrieval (keyword + decay). `withDecayed: false` = 翻旧账 mode.
#[tauri::command]
pub fn search(
    state: tauri::State<'_, MemoryStore>,
    query: String,
    limit: Option<u32>,
    with_decayed: Option<bool>,
) -> Result<Vec<MemoryHit>, String> {
    state
        .inner()
        .with_read(|conn| {
            db::search(
                conn,
                &query,
                limit.unwrap_or(20).max(1) as usize,
                with_decayed.unwrap_or(true),
            )
        })
        .map_err(|e| e.to_string())
}

/// Manual consolidation (frontend button / debugging). Uses the local LLM
/// when reachable, rules otherwise (plan §5.2). The nightly schedule lives
/// in `scheduler::spawn`.
#[tauri::command]
pub fn consolidate(state: tauri::State<'_, MemoryStore>) -> Result<consolidate::Report, String> {
    let window_end = clock::now_iso();
    // Store-backed entry: extraction runs with zero locks held.
    consolidate::run_with_local_or_rules(
        state.inner(),
        &consolidate::Options {
            trigger: "manual",
            window_start: "1970-01-01T00:00:00Z",
            window_end: &window_end,
        },
    )
    .map_err(|e| e.to_string())
}

/// Compliance Art. 16: forget a whole conversation by session id.
#[tauri::command]
pub fn forget_session(
    state: tauri::State<'_, MemoryStore>,
    session_id: String,
) -> Result<usize, String> {
    state
        .inner()
        .with_write(|conn| db::forget_session(conn, &session_id))
        .map_err(|e| e.to_string())
}

/// Compliance Art. 16: forget everything.
#[tauri::command]
pub fn forget_all(state: tauri::State<'_, MemoryStore>) -> Result<(), String> {
    state
        .inner()
        .with_write(|conn| db::forget_all(conn))
        .map_err(|e| e.to_string())
}

/// 记忆页总览（spec-m3）：只读快照，`limit` 缺省 20、上限 100。
#[tauri::command]
pub fn memory_overview(
    state: tauri::State<'_, MemoryStore>,
    limit: Option<u32>,
) -> Result<overview::MemoryOverview, String> {
    state
        .inner()
        .with_read(|conn| overview::build(conn, limit))
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod store_lock_tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn open_sets_busy_timeout_and_readers() {
        let dir = std::env::temp_dir().join(format!(
            "bonsai-store-lock-{}.sqlite3",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&dir);
        let store = MemoryStore::open(&dir).expect("open");
        assert_eq!(store.read.conns.len(), READER_COUNT);
        store
            .with_write(|conn| {
                let ms: i64 = conn.query_row("PRAGMA busy_timeout", [], |r| r.get(0))?;
                assert!(ms >= 5000, "busy_timeout={ms}");
                Ok(())
            })
            .unwrap();
        store
            .with_read(|conn| {
                // query_only 下 SELECT 可行。
                let n: i64 = conn.query_row("SELECT COUNT(*) FROM settings", [], |r| r.get(0))?;
                assert!(n >= 0);
                Ok(())
            })
            .unwrap();
        let _ = std::fs::remove_file(&dir);
        let _ = std::fs::remove_file(PathBuf::from(format!("{}-wal", dir.display())));
        let _ = std::fs::remove_file(PathBuf::from(format!("{}-shm", dir.display())));
    }

    #[test]
    fn from_connection_read_falls_back_to_write() {
        let conn = Connection::open_in_memory().unwrap();
        db::init_schema_for_test(&conn).unwrap();
        let store = MemoryStore::from_connection(conn);
        store
            .with_read(|c| {
                let _: i64 = c.query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))?;
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn reader_rejects_writes() {
        let dir = std::env::temp_dir().join(format!(
            "bonsai-store-ro-{}.sqlite3",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&dir);
        let store = MemoryStore::open(&dir).expect("open");
        let err = store
            .with_read(|conn| {
                conn.execute(
                    "INSERT INTO settings(key, value) VALUES('x', 'y')",
                    [],
                )?;
                Ok(())
            })
            .expect_err("query_only must reject writes");
        let msg = err.to_string();
        assert!(
            msg.to_lowercase().contains("readonly")
                || msg.to_lowercase().contains("read-only")
                || msg.contains("query_only")
                || msg.contains("898"),
            "unexpected err: {msg}"
        );
        let _ = std::fs::remove_file(&dir);
    }
}

//! 统一 Forget Service（docs/zh/plan-reliability-phase1/v1 R1）。
//!
//! Session / Event / All 的级联删除只住这里；`db::forget_*` 薄封装委托。

use rusqlite::{params, Connection};

use super::clock;
use super::evidence::{self, EvidenceRef, TYPE_BOUNDARY, TYPE_FACT, TYPE_PREFERENCE, TYPE_PROFILE};

/// 删除一组事件及其**仅由这些事件支撑**的派生记忆（同事务由调用方或本函数开启）。
pub fn forget_events(conn: &Connection, event_ids: &[i64]) -> rusqlite::Result<usize> {
    if event_ids.is_empty() {
        return Ok(0);
    }

    let mut derived = evidence::memories_for_events(conn, event_ids)?;
    // boundary → fact：边界行仍 FK 指向 facts，必须先删边界再删事实。
    derived.sort_by_key(|m| match m.memory_type.as_str() {
        TYPE_BOUNDARY => 0,
        TYPE_PREFERENCE => 1,
        TYPE_PROFILE => 2,
        TYPE_FACT => 3,
        _ => 4,
    });
    evidence::unlink_events(conn, event_ids)?;

    for item in derived {
        if evidence::has_any(conn, &item.memory_type, &item.memory_id)? {
            continue;
        }
        delete_orphan_memory(conn, &item)?;
    }

    // 兼容：尚无 evidence 行、仅靠 facts.source_event_id 挂着的遗留 fact。
    let placeholders = std::iter::repeat("?")
        .take(event_ids.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!("DELETE FROM facts WHERE source_event_id IN ({placeholders})");
    conn.execute(&sql, rusqlite::params_from_iter(event_ids.iter()))?;

    // 保留下来的行（如 manual profile、多源 fact）不得再 FK 指向即将删除的 event。
    let sql = format!(
        "UPDATE facts SET source_event_id = NULL WHERE source_event_id IN ({placeholders})"
    );
    conn.execute(&sql, rusqlite::params_from_iter(event_ids.iter()))?;
    let sql = format!(
        "UPDATE user_profile SET source_event_id = NULL WHERE source_event_id IN ({placeholders})"
    );
    conn.execute(&sql, rusqlite::params_from_iter(event_ids.iter()))?;

    let sql = format!("DELETE FROM events WHERE id IN ({placeholders})");
    let n = conn.execute(&sql, rusqlite::params_from_iter(event_ids.iter()))?;
    Ok(n)
}

pub fn forget_event(conn: &Connection, event_id: i64) -> rusqlite::Result<usize> {
    forget_events(conn, &[event_id])
}

/// 忘掉整段会话：派生记忆 → events → trace / tasks / sessions 行。
pub fn forget_session(conn: &Connection, session_id: &str) -> rusqlite::Result<usize> {
    let event_ids: Vec<i64> = {
        let mut stmt = conn.prepare("SELECT id FROM events WHERE session_id = ?1")?;
        let rows = stmt.query_map(params![session_id], |r| r.get(0))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };

    let n = forget_events(conn, &event_ids)?;

    // 老库未必开着 FK 级联，trace_steps 显式按 turn 删。
    conn.execute(
        "DELETE FROM trace_steps WHERE turn_id IN
             (SELECT id FROM turn_trace WHERE session_id = ?1)",
        params![session_id],
    )?;
    conn.execute(
        "DELETE FROM turn_trace WHERE session_id = ?1",
        params![session_id],
    )?;
    conn.execute("DELETE FROM tasks WHERE session_id = ?1", params![session_id])?;
    conn.execute("DELETE FROM sessions WHERE id = ?1", params![session_id])?;
    Ok(n)
}

/// 核按钮：清空各层 + evidence + 配额表。
pub fn forget_all(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "DELETE FROM memory_evidence;
         DELETE FROM proactive_daily_usage;
         DELETE FROM trace_steps; DELETE FROM turn_trace; DELETE FROM tasks;
         DELETE FROM sessions; DELETE FROM tool_calls; DELETE FROM calendar_cache;
         DELETE FROM user_profile_history; DELETE FROM user_profile;
         DELETE FROM relationship_log; DELETE FROM relationship_progress;
         DELETE FROM relationship;
         DELETE FROM ability_log; DELETE FROM ability_progress; DELETE FROM ability;
         DELETE FROM onboarding_answers; DELETE FROM settings;
         DELETE FROM growth_log; DELETE FROM growth; DELETE FROM predictions;
         DELETE FROM boundaries; DELETE FROM preferences; DELETE FROM facts;
         DELETE FROM events;",
    )?;
    Ok(())
}

fn delete_orphan_memory(conn: &Connection, item: &EvidenceRef) -> rusqlite::Result<()> {
    match item.memory_type.as_str() {
        TYPE_FACT => {
            let id: i64 = item.memory_id.parse().unwrap_or(-1);
            if id >= 0 {
                conn.execute("DELETE FROM facts WHERE id = ?1", params![id])?;
            }
        }
        TYPE_PREFERENCE => {
            let id: i64 = item.memory_id.parse().unwrap_or(-1);
            if id >= 0 {
                conn.execute("DELETE FROM preferences WHERE id = ?1", params![id])?;
            }
        }
        TYPE_BOUNDARY => {
            let id: i64 = item.memory_id.parse().unwrap_or(-1);
            if id >= 0 {
                conn.execute("DELETE FROM boundaries WHERE id = ?1", params![id])?;
            }
        }
        TYPE_PROFILE => {
            // manual 画像永不因 Session/Event Forget 删除（计划 I3 / D2）。
            let source: Option<String> = conn
                .query_row(
                    "SELECT source FROM user_profile WHERE key = ?1",
                    params![item.memory_id],
                    |r| r.get(0),
                )
                .ok();
            if source.as_deref() == Some("manual") {
                return Ok(());
            }
            if let Some(src) = source {
                let value: String = conn.query_row(
                    "SELECT value FROM user_profile WHERE key = ?1",
                    params![item.memory_id],
                    |r| r.get(0),
                )?;
                conn.execute(
                    "INSERT INTO user_profile_history (key, value, source, valid_until)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![item.memory_id, value, src, clock::now_iso()],
                )?;
                conn.execute(
                    "DELETE FROM user_profile WHERE key = ?1",
                    params![item.memory_id],
                )?;
            }
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::db::{self, add_event_meta, add_fact, promote_fact};
    use crate::memory::profile;

    fn mem() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        db::init_schema_for_test(&conn).unwrap();
        conn
    }

    /// M06：单会话唯一来源的 pref / boundary / profile 随会话删除。
    #[test]
    fn m06_forget_session_clears_sole_source_derived() {
        let conn = mem();
        let e1 = add_event_meta(&conn, "dialogue", "我对花生过敏", Some("user"), Some("s1"))
            .unwrap();
        let fact = add_fact(&conn, "用户对花生过敏", Some(e1)).unwrap();
        db::add_boundary(&conn, "不准推荐含花生的食物", Some(fact)).unwrap();
        assert!(promote_fact(&conn, "like", "用户对花生过敏", 0.9, &[e1]).unwrap());
        profile::apply(&conn, "tools", "Rust", "rule", Some(e1), 0.8).unwrap();

        forget_session(&conn, "s1").unwrap();

        let prefs: i64 = conn
            .query_row("SELECT COUNT(*) FROM preferences", [], |r| r.get(0))
            .unwrap();
        let bounds: i64 = conn
            .query_row("SELECT COUNT(*) FROM boundaries", [], |r| r.get(0))
            .unwrap();
        let facts: i64 = conn
            .query_row("SELECT COUNT(*) FROM facts", [], |r| r.get(0))
            .unwrap();
        let profiles: i64 = conn
            .query_row("SELECT COUNT(*) FROM user_profile", [], |r| r.get(0))
            .unwrap();
        assert_eq!(prefs, 0);
        assert_eq!(bounds, 0);
        assert_eq!(facts, 0);
        assert_eq!(profiles, 0);
        let events: i64 = conn
            .query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))
            .unwrap();
        assert_eq!(events, 0);
    }

    /// M08：两会话共享偏好，删一会话后保留。
    #[test]
    fn m08_shared_preference_survives_one_session_forget() {
        let conn = mem();
        let a = add_event_meta(&conn, "dialogue", "我喜欢雨天", Some("user"), Some("s-a")).unwrap();
        let b = add_event_meta(&conn, "dialogue", "还是喜欢雨天", Some("user"), Some("s-b")).unwrap();
        add_fact(&conn, "用户喜欢雨天", Some(a)).unwrap();
        add_fact(&conn, "用户喜欢雨天", Some(b)).unwrap();
        assert!(promote_fact(&conn, "like", "用户喜欢雨天", 0.9, &[a]).unwrap());
        // 第二次升格：同 content 合并补写 evidence
        assert!(!promote_fact(&conn, "like", "用户喜欢雨天", 0.9, &[b]).unwrap());

        forget_session(&conn, "s-a").unwrap();

        let prefs: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM preferences WHERE content = '用户喜欢雨天' AND invalid_at IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(prefs, 1);
        let ev: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM memory_evidence WHERE memory_type = 'preference'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(ev, 1);
    }

    #[test]
    fn manual_profile_survives_session_forget() {
        let conn = mem();
        let e = add_event_meta(&conn, "dialogue", "我叫测试", Some("user"), Some("s1")).unwrap();
        profile::apply(&conn, "name", "测试", "manual", Some(e), 1.0).unwrap();
        forget_session(&conn, "s1").unwrap();
        let name: String = conn
            .query_row(
                "SELECT value FROM user_profile WHERE key = 'name'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(name, "测试");
    }

    /// 孤儿 preference（无 evidence）在 Session Forget 时保留。
    #[test]
    fn orphan_preference_kept_on_session_forget() {
        let conn = mem();
        let e = add_event_meta(&conn, "dialogue", "hi", Some("user"), Some("s1")).unwrap();
        add_fact(&conn, "无关事实", Some(e)).unwrap();
        db::add_preference(&conn, "habit", "历史孤儿偏好", 0.6).unwrap();
        forget_session(&conn, "s1").unwrap();
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM preferences WHERE content = '历史孤儿偏好'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
    }

    /// M07：删除后 FTS / search 不再召回该会话内容。
    #[test]
    fn m07_forget_session_clears_fts_hits() {
        let conn = mem();
        let e = add_event_meta(
            &conn,
            "dialogue",
            "我在学独特词紫水晶编年",
            Some("user"),
            Some("s1"),
        )
        .unwrap();
        add_fact(&conn, "用户在学独特词紫水晶编年", Some(e)).unwrap();
        let hits = db::search(&conn, "紫水晶编年", 10, true).unwrap();
        assert!(!hits.is_empty(), "删除前应能搜到");
        forget_session(&conn, "s1").unwrap();
        let hits = db::search(&conn, "紫水晶编年", 10, true).unwrap();
        assert!(hits.is_empty(), "删除后 FTS/search 不得残留: {hits:?}");
    }
}

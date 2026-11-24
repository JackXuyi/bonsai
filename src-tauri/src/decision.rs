//! L4 — Decision layer: "should it speak?" (report §6.4 — the easiest place
//! to get it wrong).
//!
//! Research constraints:
//! - Proactivity is the most expensive move. Default cap: 1 proactive message
//!   per day, and only when idle > 5 minutes.
//! - Interruption-cost scoring: speak only when `value - interruption_cost >
//!   threshold`; typing / fullscreen / meeting → always silent.
//! - Treat "silence" as an explicit action and reward correct silence; an
//!   ignored notification is a strong negative sample.
//! - The decision layer is independent of the generation layer: whether to
//!   speak ≠ what to say.
//!
//! Quota（plan-reliability-phase1 R4）：权威在 SQLite `proactive_daily_usage`，
//! 本地日历日；`try_reserve` → 成功发出后 `commit_speak`，失败 `release_reserve`。

use rusqlite::Connection;
use serde::Serialize;

use crate::memory::clock;

/// 进程内单飞锁占位（配额权威在 SQLite；此结构避免双线程同时过 Gate）。
#[derive(Debug, Default)]
pub struct ProactivityState;

#[derive(Debug, Clone)]
pub struct DecisionInput {
    pub idle_minutes: u32,
    pub is_typing: bool,
    pub is_fullscreen: bool,
    pub in_meeting: bool,
    /// How valuable is this message to the user, 0.0–1.0.
    pub value: f32,
    /// How much would it cost to interrupt right now, 0.0–1.0.
    pub interruption_cost: f32,
}

#[derive(Debug, PartialEq, Serialize)]
pub enum Decision {
    Speak,
    #[serde(rename_all = "snake_case")]
    Silent(SilentReason),
}

#[derive(Debug, PartialEq, Serialize)]
pub enum SilentReason {
    /// Typing / fullscreen / meeting — never interrupt.
    Busy,
    /// Daily quota exhausted (default: 1/day).
    QuotaExhausted,
    /// User is still active; wait for idle > 5 minutes.
    UserActive,
    /// Value does not outweigh the interruption cost.
    NotWorthIt,
}

const MIN_IDLE_MINUTES: u32 = 5;
/// Default daily proactivity cap (report §6.4: 1 proactive message per day).
pub const DEFAULT_DAILY_LIMIT: u32 = 1;

fn local_date_key() -> String {
    clock::now_local().date_naive().format("%Y-%m-%d").to_string()
}

/// 忙碌 / 空闲 / 价值闸（不碰配额）。
pub fn gate_interruptibility(input: &DecisionInput) -> Option<SilentReason> {
    if input.is_typing || input.is_fullscreen || input.in_meeting {
        return Some(SilentReason::Busy);
    }
    if input.idle_minutes < MIN_IDLE_MINUTES {
        return Some(SilentReason::UserActive);
    }
    if input.value - input.interruption_cost <= 0.0 {
        return Some(SilentReason::NotWorthIt);
    }
    None
}

/// 原子预留一日配额。成功返回 true（已 reserved+1）。
pub fn try_reserve(conn: &Connection, daily_limit: u32) -> rusqlite::Result<bool> {
    let day = local_date_key();
    let limit = daily_limit.max(1) as i64;
    conn.execute(
        "INSERT INTO proactive_daily_usage (local_date, used_count, reserved)
         VALUES (?1, 0, 1)
         ON CONFLICT(local_date) DO UPDATE SET
           reserved = reserved + 1
         WHERE used_count + reserved < ?2",
        rusqlite::params![day, limit],
    )?;
    Ok(conn.changes() > 0)
}

/// 主动消息已成功落库/发出后提交消耗。
pub fn commit_speak(conn: &Connection) -> rusqlite::Result<()> {
    let day = local_date_key();
    let now = clock::now_iso();
    conn.execute(
        "UPDATE proactive_daily_usage
            SET used_count = used_count + 1,
                reserved = CASE WHEN reserved > 0 THEN reserved - 1 ELSE 0 END,
                last_spoken_at = ?2
          WHERE local_date = ?1",
        rusqlite::params![day, now],
    )?;
    Ok(())
}

/// 生成失败时释放预留。
pub fn release_reserve(conn: &Connection) -> rusqlite::Result<()> {
    let day = local_date_key();
    conn.execute(
        "UPDATE proactive_daily_usage
            SET reserved = CASE WHEN reserved > 0 THEN reserved - 1 ELSE 0 END
          WHERE local_date = ?1",
        rusqlite::params![day],
    )?;
    Ok(())
}

/// 启动时作废未提交预留（崩溃残留）。
pub fn clear_stale_reserves(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute("UPDATE proactive_daily_usage SET reserved = 0", [])?;
    Ok(())
}

/// Gate + 配额预留。`Speak` 表示已预留，调用方必须 commit 或 release。
pub fn should_speak(
    input: &DecisionInput,
    conn: &Connection,
    daily_limit: u32,
) -> rusqlite::Result<Decision> {
    if let Some(reason) = gate_interruptibility(input) {
        return Ok(Decision::Silent(reason));
    }
    if !try_reserve(conn, daily_limit)? {
        return Ok(Decision::Silent(SilentReason::QuotaExhausted));
    }
    Ok(Decision::Speak)
}

/// Shared L4 evaluation: latest L1 signals + today's perception volume →
/// `should_speak`. Used by the command below and by the proactive loop.
pub fn evaluate_live(
    conn: &Connection,
    signals: &crate::perception::LatestSignals,
    _proactivity: &mut ProactivityState,
) -> Decision {
    let day = clock::now_local().date_naive();
    let signals_today = crate::report::local_day_window(day)
        .and_then(|(start, end)| {
            crate::memory::db::events_in_window(conn, &start, &end).ok()
        })
        .map(|events| events.iter().filter(|e| e.kind == "perception").count())
        .unwrap_or(0);
    let input = crate::perception::decision_input(
        signals,
        signals_today,
        crate::calendar::in_meeting_now(conn, clock::now_local()),
    );
    should_speak(&input, conn, DEFAULT_DAILY_LIMIT).unwrap_or(Decision::Silent(SilentReason::Busy))
}

/// L4 entry: run `should_speak` on the latest L1 signals.
#[tauri::command]
pub fn evaluate(
    store: tauri::State<'_, crate::memory::MemoryStore>,
    perception: tauri::State<'_, crate::perception::PerceptionState>,
    app_state: tauri::State<'_, crate::AppState>,
) -> Result<Decision, String> {
    let signals = perception
        .inner()
        .latest
        .lock()
        .map_err(|e| e.to_string())?
        .clone();
    let mut proactivity = app_state.proactivity.lock().map_err(|e| e.to_string())?;
    store
        .inner()
        .with_write(|conn| Ok(evaluate_live(conn, &signals, &mut proactivity)))
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::db;

    fn mem() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        db::init_schema_for_test(&conn).unwrap();
        conn
    }

    fn input(idle: u32, value: f32, cost: f32) -> DecisionInput {
        DecisionInput {
            idle_minutes: idle,
            is_typing: false,
            is_fullscreen: false,
            in_meeting: false,
            value,
            interruption_cost: cost,
        }
    }

    #[test]
    fn busy_beats_everything() {
        let conn = mem();
        let mut i = input(30, 0.9, 0.0);
        i.is_typing = true;
        assert_eq!(
            should_speak(&i, &conn, 1).unwrap(),
            Decision::Silent(SilentReason::Busy)
        );
    }

    #[test]
    fn quota_limits_to_one_per_day() {
        let conn = mem();
        let i = input(10, 0.9, 0.0);
        assert_eq!(
            should_speak(&i, &conn, DEFAULT_DAILY_LIMIT).unwrap(),
            Decision::Speak
        );
        assert_eq!(
            should_speak(&i, &conn, DEFAULT_DAILY_LIMIT).unwrap(),
            Decision::Silent(SilentReason::QuotaExhausted)
        );
    }

    #[test]
    fn requires_idle_over_five_minutes() {
        let conn = mem();
        assert_eq!(
            should_speak(&input(4, 0.9, 0.0), &conn, 1).unwrap(),
            Decision::Silent(SilentReason::UserActive)
        );
        assert_eq!(
            should_speak(&input(5, 0.9, 0.0), &conn, 1).unwrap(),
            Decision::Speak
        );
    }

    #[test]
    fn value_must_outweigh_cost() {
        let conn = mem();
        assert_eq!(
            should_speak(&input(30, 0.3, 0.5), &conn, 1).unwrap(),
            Decision::Silent(SilentReason::NotWorthIt)
        );
        assert_eq!(
            should_speak(&input(30, 0.8, 0.2), &conn, 1).unwrap(),
            Decision::Speak
        );
    }

    /// D01：配额键为本地日（pin 时钟）。
    #[test]
    fn d01_quota_uses_local_calendar_day() {
        let _g = clock::pinned("2026-11-23T16:00:00Z"); // 本地 11-24 00:00 CST
        let conn = mem();
        let i = input(10, 0.9, 0.0);
        assert_eq!(should_speak(&i, &conn, 1).unwrap(), Decision::Speak);
        let day: String = conn
            .query_row(
                "SELECT local_date FROM proactive_daily_usage LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(day, clock::now_local().date_naive().format("%Y-%m-%d").to_string());
    }

    /// D02：提交后「重启」（新连接读同库文件语义：同表）不可再 Speak。
    #[test]
    fn d02_quota_persists_across_state_reset() {
        let conn = mem();
        let i = input(10, 0.9, 0.0);
        assert_eq!(should_speak(&i, &conn, 1).unwrap(), Decision::Speak);
        commit_speak(&conn).unwrap();
        // 新 ProactivityState 不影响 SQLite 权威。
        let _fresh = ProactivityState::default();
        assert_eq!(
            should_speak(&i, &conn, 1).unwrap(),
            Decision::Silent(SilentReason::QuotaExhausted)
        );
    }

    /// D03：reserve 后失败 release，仍可再次 Speak。
    #[test]
    fn d03_release_allows_retry() {
        let conn = mem();
        let i = input(10, 0.9, 0.0);
        assert_eq!(should_speak(&i, &conn, 1).unwrap(), Decision::Speak);
        release_reserve(&conn).unwrap();
        assert_eq!(should_speak(&i, &conn, 1).unwrap(), Decision::Speak);
    }
}

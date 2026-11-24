//! P2 item 1 — the relationship line (docs/zh/plan-relationship.md).
//!
//! The plant changes SHAPE in the first week (morph.rs). The relationship
//! changes how it TALKS from day 9 on: 拘谨 → 熟络 → 会顶嘴. It advances on
//! **understanding accuracy**, not time and not check-ins (report §6.3
//! decision 2): the only evidence is "it guessed right what you were up to,
//! and you let it guess".
//!
//! Invariants (plan §3.4):
//! - Stage only increases. Going cold lowers the *temperature*, never the stage.
//! - Evidence is RECOMPUTED daily from predictions + events, never accumulated
//!   by a counter — same sources, same score (replayable, self-correcting).
//! - The numeric score NEVER leaves this process: no API field, no UI, no log.
//! - `shut_up` never subtracts points: clicking "别烦我" is the user exercising
//!   control, and punishing it trains them to stop giving feedback. It only
//!   pauses promotion for a while (better silent than annoying).

use chrono::{DateTime, Duration, NaiveDate, TimeZone, Utc};
use rusqlite::Connection;
use serde::Serialize;

use crate::memory::db;
use crate::memory::clock;

/// (min_day, min_cumulative_evidence) per stage. Both must hold — time only
/// provides the window, never the progress (plan §3.1).
pub const STAGE_THRESHOLDS: [(i64, i64); 3] = [(0, 0), (9, 12), (22, 30)];

pub const STAGE_NAMES: [&str; 3] = ["拘谨", "熟络", "会顶嘴"];

/// Days without any activity before the "rusty" mood sets in. Only applies
/// once there is something to be rusty about (stage >= 1).
pub const COLD_AFTER_DAYS: i64 = 7;

/// Recent back-off above this rate pauses promotion (plan §3.4).
pub const BACKOFF_PAUSE_RATE: f64 = 0.10;

/// Window for both the back-off rate and the cumulative evidence.
pub const EVIDENCE_WINDOW_DAYS: i64 = 30;
pub const BACKOFF_WINDOW_DAYS: i64 = 7;

/// The "first refusal" moment is only eligible this soon after R2 unlocks.
pub const FIRST_REFUSAL_WINDOW_DAYS: i64 = 7;

/// "The user is around" — same definition as the form line. Derived events
/// (disagreement / relationship_*) are deliberately excluded: counting our
/// own writes as activity would mean "keeping itself company", and the plant
/// already taught us this lesson with `face_state`.
pub const ACTIVITY_KINDS: &[&str] = &["dialogue", "perception", "prediction_asked", "prediction_feedback"];

// ---------------------------------------------------------------- derivation

/// Daily evidence score (plan §3.2). Three positive signals, each capped at
/// +1; one gentle negative capped at -1. `shut_ups` never subtracts.
pub fn score_day(present: bool, hits: i64, misses: i64, disagreements: i64, shut_ups: i64) -> i64 {
    let mut s = 0;
    if present {
        s += 1;
    }
    if hits > 0 {
        s += 1;
    }
    if disagreements > 0 && shut_ups == 0 {
        s += 1;
    }
    if misses > 0 {
        s -= 1;
    }
    s
}

/// Pure promotion rule: BOTH the day threshold and the cumulative evidence
/// must be met. `cum` excludes today (plan §3.3) — one lucky guess at noon
/// must not promote the relationship mid-day.
pub fn derive_stage(day_index: i64, cum: i64) -> i64 {
    let mut stage = 0;
    for (i, (min_day, min_cum)) in STAGE_THRESHOLDS.iter().enumerate() {
        if day_index >= *min_day && cum >= *min_cum {
            stage = i as i64;
        }
    }
    stage
}

/// The prompt-facing mode. Cold overrides the stage: a rusty friend talks
/// politely again without the relationship actually rewinding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationshipMode {
    Reserved,
    Familiar,
    Direct,
    Rusty,
}

impl RelationshipMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            RelationshipMode::Reserved => "reserved",
            RelationshipMode::Familiar => "familiar",
            RelationshipMode::Direct => "direct",
            RelationshipMode::Rusty => "rusty",
        }
    }
}

pub fn mode_of(stage: i64, cold: bool) -> RelationshipMode {
    if cold {
        return RelationshipMode::Rusty;
    }
    match stage {
        0 => RelationshipMode::Reserved,
        1 => RelationshipMode::Familiar,
        _ => RelationshipMode::Direct,
    }
}

/// Read-only convenience for the prompt layer (one query per chat turn).
pub fn current_mode(conn: &Connection) -> RelationshipMode {
    let state = db::relationship_state(conn).ok().flatten();
    let stage = state.as_ref().map(|s| s.stage).unwrap_or(0);
    let cold = state
        .as_ref()
        .map(|s| s.cold_since.is_some())
        .unwrap_or(false);
    mode_of(stage, cold)
}

/// Announcement wording. `from` matters only for the old-user backfill case
/// (0 → 2 in a single tick), which must sound like "we've been at this a
/// while", not like a double unlock.
fn announcement(from: i64, to: i64, day_index: i64) -> String {
    if from == 0 && to >= 2 {
        return "其实我们已经处了一阵子了。行吧，那就不绕弯了。".into();
    }
    let variant = (day_index.max(0) as usize) % 2;
    match (to, variant) {
        (1, 0) => "处了一阵子了。你几点犯困，我差不多摸清了。".into(),
        (1, _) => "熟了点了。以后我不总问「在吗」了。".into(),
        (2, 0) => "行吧，混熟了。以后你说错我直接讲，不绕。".into(),
        _ => "想明白了——光顺着你没意思。有不同意见我就说。".into(),
    }
}

// ---------------------------------------------------------------- evidence

/// One local day's raw evidence, derived fresh from the source tables.
pub fn day_evidence(
    conn: &Connection,
    day: NaiveDate,
    now_local: DateTime<chrono::Local>,
) -> rusqlite::Result<db::DayEvidence> {
    // Predictions carry their own local `date` column — no parsing needed.
    let (hits, misses, shut_ups): (i64, i64, i64) = conn.query_row(
        "SELECT
            COALESCE(SUM(CASE WHEN feedback = 'hit' THEN 1 ELSE 0 END), 0),
            COALESCE(SUM(CASE WHEN feedback = 'miss' THEN 1 ELSE 0 END), 0),
            COALESCE(SUM(CASE WHEN feedback = 'shut_up' THEN 1 ELSE 0 END), 0)
         FROM predictions WHERE date = ?1",
        rusqlite::params![day.to_string()],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;

    // Events only carry an RFC3339 UTC stamp — convert the local day to a
    // UTC half-open range and let SQL filter, then judge kinds in Rust.
    let start = local_midnight(day, now_local.offset()).to_rfc3339();
    let end = local_midnight(day + Duration::days(1), now_local.offset()).to_rfc3339();
    let mut stmt = conn.prepare(
        "SELECT kind FROM events
         WHERE ts >= ?1 AND ts < ?2
           AND kind IN ('dialogue', 'prediction_feedback', 'disagreement')",
    )?;
    let mut present = false;
    let mut disagreements = 0i64;
    let rows = stmt.query_map(rusqlite::params![start, end], |r| r.get::<_, String>(0))?;
    for kind in rows {
        match kind?.as_str() {
            "dialogue" | "prediction_feedback" => present = true,
            "disagreement" => disagreements += 1,
            _ => {}
        }
    }

    Ok(db::DayEvidence {
        present: i64::from(present),
        hits,
        misses,
        disagreements,
        shut_ups,
        score: score_day(present, hits, misses, disagreements, shut_ups),
    })
}

fn local_midnight(
    day: NaiveDate,
    offset: &chrono::FixedOffset,
) -> DateTime<Utc> {
    let naive = day.and_hms_opt(0, 0, 0).expect("midnight is always valid");
    offset
        .from_local_datetime(&naive)
        .earliest()
        .map(|t| t.with_timezone(&Utc))
        .unwrap_or_else(Utc::now)
}

/// `shut_up / asked` over the last `days` — parsed in Rust because mixing
/// RFC3339 strings with SQLite datetime() has burned us before.
pub fn backoff_rate_recent(
    conn: &Connection,
    now_local: DateTime<chrono::Local>,
    days: i64,
) -> rusqlite::Result<Option<f64>> {
    let cutoff = now_local.with_timezone(&Utc) - Duration::days(days);
    let mut stmt = conn.prepare(
        "SELECT asked_at, feedback FROM predictions WHERE asked_at IS NOT NULL",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
    })?;
    let (mut asked, mut shut) = (0usize, 0usize);
    for row in rows {
        let (asked_at, feedback) = row?;
        let Ok(t) = DateTime::parse_from_rfc3339(&asked_at) else {
            continue;
        };
        if t.with_timezone(&Utc) >= cutoff {
            asked += 1;
            if feedback.as_deref() == Some("shut_up") {
                shut += 1;
            }
        }
    }
    if asked == 0 {
        Ok(None)
    } else {
        Ok(Some(shut as f64 / asked as f64))
    }
}

/// Recompute + store the last `days` days (plan §3.7 backfill). Idempotent by
/// construction, so it also runs on every boot as a self-healing pass: the
/// window rolls forward and any stale day gets corrected from the sources.
pub fn backfill(
    conn: &Connection,
    now_local: DateTime<chrono::Local>,
    days: i64,
) -> rusqlite::Result<usize> {
    let today = now_local.date_naive();
    let mut n = 0;
    for back in 0..days {
        let day = today - Duration::days(back);
        let ev = day_evidence(conn, day, now_local)?;
        db::upsert_progress(conn, &day.to_string(), &ev)?;
        n += 1;
    }
    Ok(n)
}

// ---------------------------------------------------------------- tick

#[derive(Debug, Default, PartialEq)]
pub struct TickOutcome {
    /// (stage, note) when a new stage unlocked this tick.
    pub unlocked: Option<(i64, String)>,
    /// Some(true) = went cold · Some(false) = warmed up · None = no change.
    pub cold_changed: Option<bool>,
    pub stage: i64,
    pub day_index: i64,
    pub paused: bool,
}

pub fn tick(
    conn: &Connection,
    now_local: DateTime<chrono::Local>,
) -> rusqlite::Result<TickOutcome> {
    let state = db::ensure_relationship(conn)?;
    let today = now_local.format("%Y-%m-%d").to_string();
    let today_naive = now_local.date_naive();
    let mut out = TickOutcome {
        stage: state.stage,
        ..Default::default()
    };

    // ① Recompute today's evidence on EVERY tick. Sources unchanged → same
    //    row (idempotent); sources changed → the day corrects itself.
    let ev = day_evidence(conn, today_naive, now_local)?;
    db::upsert_progress(conn, &today, &ev)?;

    // ② Promotion: once per local day, double condition, pause on back-off.
    if state.last_tick_date.as_deref() != Some(today.as_str()) {
        let growth = db::ensure_growth(conn)?;
        let di = crate::morph::day_index(&growth.born_at, now_local);
        let from = today_naive - Duration::days(EVIDENCE_WINDOW_DAYS);
        let cum = db::progress_sum_between(conn, &from.to_string(), &today)?;
        let backoff = backoff_rate_recent(conn, now_local, BACKOFF_WINDOW_DAYS)?
            .unwrap_or(0.0);

        out.day_index = di;
        out.paused = backoff > BACKOFF_PAUSE_RATE;

        if !out.paused {
            let target = derive_stage(di, cum);
            if target > state.stage {
                let note = announcement(state.stage, target, di);
                db::set_relationship_stage(conn, target)?;
                db::add_relationship_log(conn, target, &note)?;
                db::add_event(
                    conn,
                    "relationship_stage",
                    &format!("{{\"stage\":{target},\"day\":{di}}}"),
                )?;
                out.unlocked = Some((target, note));
            }
        }
        db::set_relationship_tick_date(conn, &today)?;
        out.stage = db::relationship_state(conn)?
            .map(|s| s.stage)
            .unwrap_or(state.stage);
    } else {
        let growth = db::ensure_growth(conn)?;
        out.day_index = crate::morph::day_index(&growth.born_at, now_local);
    }

    // ③ Cold: only meaningful once there is a relationship (stage >= 1).
    //    A brand-new install is "polite", not "abandoned" — same guard as
    //    the form line's dormancy.
    let cutoff = (now_local.with_timezone(&Utc) - Duration::days(COLD_AFTER_DAYS)).to_rfc3339();
    let kinds = ACTIVITY_KINDS
        .iter()
        .map(|k| format!("'{k}'"))
        .collect::<Vec<_>>()
        .join(", ");
    let active: i64 = conn.query_row(
        &format!("SELECT COUNT(*) FROM events WHERE ts >= ?1 AND kind IN ({kinds})"),
        rusqlite::params![cutoff],
        |r| r.get(0),
    )?;
    let historical: i64 = conn.query_row(
        &format!("SELECT COUNT(*) FROM events WHERE ts < ?1 AND kind IN ({kinds})"),
        rusqlite::params![cutoff],
        |r| r.get(0),
    )?;
    let cold = out.stage >= 1 && active == 0 && historical > 0;
    let changed = if cold {
        db::set_relationship_cold(conn, Some(&clock::now_iso()))? > 0
    } else {
        db::set_relationship_cold(conn, None)? > 0
    };
    if changed {
        out.cold_changed = Some(cold);
        db::add_event(
            conn,
            "relationship_cold",
            if cold { "on" } else { "off" },
        )?;
    }
    Ok(out)
}

// ---------------------------------------------------------------- first refusal

/// Called when the anti-sycophancy filter sees a HEDGED disagreement — the
/// model wanted to push back but softened it. If this happens inside the
/// 7-day window after R2 and the user did not wave it off that day, it is
/// the moment the relationship becomes real: "I wanted to agree, then
/// thought better of it." Fires at most once, ever.
pub fn maybe_first_refusal(
    conn: &Connection,
    now_local: DateTime<chrono::Local>,
) -> rusqlite::Result<bool> {
    let Some(state) = db::relationship_state(conn)? else {
        return Ok(false);
    };
    if state.stage < 2 {
        return Ok(false);
    }
    let Some(since) = state.stage_since.as_deref() else {
        return Ok(false);
    };
    let Ok(since) = DateTime::parse_from_rfc3339(since) else {
        return Ok(false);
    };
    if now_local.with_timezone(&Utc) - since.with_timezone(&Utc)
        > Duration::days(FIRST_REFUSAL_WINDOW_DAYS)
    {
        return Ok(false);
    }
    // Only once, ever.
    let done: i64 = conn.query_row(
        "SELECT COUNT(*) FROM relationship_log WHERE kind = 'first_refusal'",
        [],
        |r| r.get(0),
    )?;
    if done > 0 {
        return Ok(false);
    }
    // The user must not have waved anything off that day.
    let ev = day_evidence(conn, now_local.date_naive(), now_local)?;
    if ev.shut_ups > 0 {
        return Ok(false);
    }
    db::add_relationship_log_kind(
        conn,
        "first_refusal",
        2,
        "我今天本来想顺着你说，想了想——不。",
    )?;
    Ok(true)
}

// ---------------------------------------------------------------- spawn

/// Poll thread: every 60s, one relationship tick + a rolling backfill on
/// boot so a long absence is recomputed honestly.
pub fn spawn(app: tauri::AppHandle) {
    use tauri::Manager;
    // T5 §5.6/A9 + plan-loop-supervisor：双层守卫；CAP 留痕；D1 有界复活。
    crate::failure::spawn_guarded("relationship", move || {
        crate::failure::try_register(&app, "relationship");
        let mut booted = false;
        let mut pg = crate::failure::PanicGuard::new("relationship");
        loop {
            std::thread::sleep(std::time::Duration::from_secs(60));
            crate::failure::try_heartbeat(&app, "relationship");
            let Some(store) = app.try_state::<crate::memory::MemoryStore>() else {
                continue;
            };
            let Ok(conn) = store.inner().write_guard() else {
                continue;
            };
            let now = chrono::Local::now();
            if !booted {
                booted = true;
                let _ = backfill(&conn, now, EVIDENCE_WINDOW_DAYS);
            }
            let _ = pg.tick(|| match tick(&conn, now) {
                Ok(out) => {
                    if out.unlocked.is_some() || out.cold_changed.is_some() {
                        let payload = status_payload(&conn);
                        drop(conn);
                        use tauri::Emitter;
                        let _ = app.emit("relationship:stage", payload);
                    }
                }
                Err(_) => {}
            });
            if pg.stopped() {
                if crate::failure::on_loop_cap(&app, "relationship", "consecutive panic cap") {
                    spawn(app.clone());
                }
                break;
            }
        }
    });
}

// ---------------------------------------------------------------- status

/// What the frontend may know. Deliberately has NO cumulative score field:
/// the number stays in the database (plan §3.2 — no progress bar, ever).
#[derive(Debug, Clone, Serialize)]
pub struct RelationshipStatus {
    pub stage: i64,
    pub stage_name: String,
    pub day_index: i64,
    pub stage_since: Option<String>,
    pub cold: bool,
    pub paused: bool,
    pub pause_reason: Option<String>,
    pub pending: Vec<PendingRelationship>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PendingRelationship {
    pub id: i64,
    pub kind: String,
    pub stage: i64,
    pub note: String,
}

pub fn status_payload(conn: &Connection) -> RelationshipStatus {
    let state = db::ensure_relationship(conn).ok();
    let growth = db::ensure_growth(conn).ok();
    let now = chrono::Local::now();
    let day_index = growth
        .as_ref()
        .map(|g| crate::morph::day_index(&g.born_at, now))
        .unwrap_or(0);
    let stage = state.as_ref().map(|s| s.stage).unwrap_or(0);
    let paused = backoff_rate_recent(conn, now, BACKOFF_WINDOW_DAYS)
        .ok()
        .flatten()
        .map(|r| r > BACKOFF_PAUSE_RATE)
        .unwrap_or(false);
    let pending = db::pending_relationship_logs(conn)
        .unwrap_or_default()
        .into_iter()
        .map(|(id, kind, stage, note)| PendingRelationship { id, kind, stage, note })
        .collect();
    RelationshipStatus {
        stage,
        stage_name: STAGE_NAMES
            .get(stage as usize)
            .unwrap_or(&"拘谨")
            .to_string(),
        day_index,
        stage_since: state.and_then(|s| s.stage_since),
        cold: db::relationship_state(conn)
            .ok()
            .flatten()
            .map(|s| s.cold_since.is_some())
            .unwrap_or(false),
        paused,
        pause_reason: paused.then(|| "最近你常让我别烦，我先不往前凑了。".to_string()),
        pending,
    }
}

// ---------------------------------------------------------------- commands

#[tauri::command]
pub fn relationship_status(
    store: tauri::State<'_, crate::memory::MemoryStore>,
) -> Result<RelationshipStatus, String> {
    store
        .inner()
        .with_read(|conn| Ok(status_payload(conn)))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn relationship_ack(
    store: tauri::State<'_, crate::memory::MemoryStore>,
    id: i64,
) -> Result<bool, String> {
    let conn = store.inner().write_guard().map_err(|e| e.to_string())?;
    db::mark_relationship_notified(&conn, id).map_err(|e| e.to_string())
}

/// Start over socially: wipes the relationship only. Memory, the plant and
/// the onboarding answers stay (plan §3.8 — reset should be per-line).
#[tauri::command]
pub fn relationship_reset(
    store: tauri::State<'_, crate::memory::MemoryStore>,
) -> Result<(), String> {
    let conn = store.inner().write_guard().map_err(|e| e.to_string())?;
    db::forget_relationship(&conn).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::clock;

    fn mem() -> Connection {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir = std::env::temp_dir().join("bonsai-relationship-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("test-{}-{n}.sqlite3", std::process::id()));
        let _ = std::fs::remove_file(&path);
        db::open(&path).unwrap()
    }

    fn local(y: i32, m: u32, d: u32, h: u32) -> DateTime<chrono::Local> {
        chrono::Local
            .with_ymd_and_hms(y, m, d, h, 0, 0)
            .single()
            .unwrap()
    }

    /// Put the plant's birth anchor at `y-m-d` so day_index is predictable.
    fn born_on(conn: &Connection, iso: &str) {
        db::ensure_growth(conn).unwrap();
        conn.execute(
            "UPDATE growth SET born_at = ?1 WHERE id = 1",
            rusqlite::params![iso],
        )
        .unwrap();
    }

    fn predict(conn: &Connection, date: &str, feedback: &str) {
        conn.execute(
            "INSERT INTO predictions (date, guess, basis, ask_after_hour, asked_at, feedback, feedback_at)
             VALUES (?1, 'g', '[]', 10, ?2, ?3, ?2)",
            rusqlite::params![date, format!("{date}T02:00:00+00:00"), feedback],
        )
        .unwrap();
    }

    // ------------------------------------------------ pure functions

    /// Release gate ① (plan §4): promotion needs BOTH conditions — day 9 with
    /// 11 evidence points must NOT promote, day 8 with 99 must NOT promote.
    #[test]
    fn stage_needs_day_and_evidence_together() {
        assert_eq!(derive_stage(0, 0), 0);
        assert_eq!(derive_stage(8, 99), 0, "day 8 is too early no matter what");
        assert_eq!(derive_stage(9, 11), 0, "one point short is short");
        assert_eq!(derive_stage(9, 12), 1);
        assert_eq!(derive_stage(21, 99), 1, "R2 waits for day 22");
        assert_eq!(derive_stage(22, 29), 1);
        assert_eq!(derive_stage(22, 30), 2);
        assert_eq!(derive_stage(365, 999), 2, "stage caps at R2");
    }

    #[test]
    fn scoring_never_punishes_control() {
        // "别烦我" alone: no penalty, no reward (the user exercised control).
        assert_eq!(score_day(true, 0, 0, 0, 3), 1);
        // A real push-back that was not waved off is worth as much as a hit.
        assert_eq!(score_day(true, 0, 0, 1, 0), 2);
        // ...but a push-back on a day the user waved things off is not evidence.
        assert_eq!(score_day(true, 0, 0, 1, 1), 1);
        // A wrong guess costs one point, never compounds.
        assert_eq!(score_day(true, 0, 3, 0, 0), 0);
        // Bare presence still counts — showing up matters.
        assert_eq!(score_day(true, 0, 0, 0, 0), 1);
        assert_eq!(score_day(false, 0, 0, 0, 0), 0);
    }

    #[test]
    fn cold_overrides_stage_in_prompt_mode() {
        assert_eq!(mode_of(0, false), RelationshipMode::Reserved);
        assert_eq!(mode_of(1, false), RelationshipMode::Familiar);
        assert_eq!(mode_of(2, false), RelationshipMode::Direct);
        // Rusty is a temperature, not a demotion.
        assert_eq!(mode_of(2, true), RelationshipMode::Rusty);
        assert_eq!(mode_of(2, true).as_str(), "rusty");
    }

    // ------------------------------------------------ evidence from sources

    #[test]
    fn day_evidence_reads_predictions_and_events() {
        let _c = clock::pinned("2026-11-19T02:00:00+00:00");
        let conn = mem();
        let now = local(2026, 11, 19, 10);
        predict(&conn, "2026-11-19", "hit");
        db::add_event(&conn, "dialogue", "hi").unwrap();
        db::add_event(&conn, "disagreement", "我不同意这个判断").unwrap();
        db::add_event(&conn, "disagreement_hedged", "虽然…但…").unwrap();

        let ev = day_evidence(&conn, now.date_naive(), now).unwrap();
        assert_eq!(ev.hits, 1);
        assert_eq!(ev.misses, 0);
        assert_eq!(ev.disagreements, 1, "hedged does NOT count as a push-back");
        assert_eq!(ev.present, 1);
        assert_eq!(ev.score, 3, "present + hit + push-back-not-waved-off");

        // A day with nothing is worth nothing.
        let ev2 = day_evidence(&conn, now.date_naive() - Duration::days(3), now).unwrap();
        assert_eq!(ev2, db::DayEvidence::default());
    }

    /// Release gate ② (plan §4): promotion pauses above the back-off rate,
    /// and pausing never rewinds the stage.
    #[test]
    fn backoff_pauses_promotion_without_rewinding() {
        let _c = clock::pinned("2026-11-01T02:00:00+00:00");
        let conn = mem();
        born_on(&conn, "2026-11-01T02:00:00+00:00");
        // Plenty of evidence BEFORE today.
        for d in 1..=9 {
            let day = format!("2026-11-{:02}", d);
            db::upsert_progress(
                &conn,
                &day,
                &db::DayEvidence { present: 1, hits: 1, score: 2, ..Default::default() },
            )
            .unwrap();
        }
        // ...but 3 of the last 4 asks were waved off → 75% back-off.
        for d in 16..=19 {
            predict(
                &conn,
                &format!("2026-11-{d}"),
                if d == 16 { "hit" } else { "shut_up" },
            );
        }
        let _c2 = clock::pinned("2026-11-19T02:00:00+00:00");
        let now = local(2026, 11, 19, 10);
        let out = tick(&conn, now).unwrap();
        assert!(out.paused, "75% back-off must pause promotion");
        assert_eq!(out.stage, 0, "paused means paused, not promoted");
        assert_eq!(out.unlocked, None);

        // A quiet week later the pause lifts and the (still valid) evidence
        // promotes — the stage was never subtracted in the meantime.
        let _c3 = clock::pinned("2026-11-27T02:00:00+00:00");
        let out2 = tick(&conn, local(2026, 11, 27, 10)).unwrap();
        assert!(!out2.paused);
        assert!(out2.stage >= 1, "evidence survives a pause");
    }

    #[test]
    fn promotion_is_once_per_day_and_idempotent() {
        let _c = clock::pinned("2026-11-01T02:00:00+00:00");
        let conn = mem();
        born_on(&conn, "2026-11-01T02:00:00+00:00");
        for d in 1..=9 {
            db::upsert_progress(
                &conn,
                &format!("2026-11-{:02}", d),
                &db::DayEvidence { present: 1, hits: 1, score: 2, ..Default::default() },
            )
            .unwrap();
        }
        let _c2 = clock::pinned("2026-11-10T02:00:00+00:00");
        let out = tick(&conn, local(2026, 11, 10, 10)).unwrap();
        assert_eq!(out.stage, 1, "day 9 + 18 evidence → R1");
        assert!(out.unlocked.is_some());
        assert_eq!(db::pending_relationship_logs(&conn).unwrap().len(), 1);

        // Same day, another tick: no duplicate log, no new promotion.
        let out2 = tick(&conn, local(2026, 11, 10, 15)).unwrap();
        assert_eq!(out2.unlocked, None);
        assert_eq!(db::pending_relationship_logs(&conn).unwrap().len(), 1);
    }

    /// Today's evidence must not promote mid-day (plan §3.3): cum excludes
    /// today, so a single lucky hit at noon changes nothing.
    #[test]
    fn today_never_counts_toward_promotion() {
        let _c = clock::pinned("2026-11-01T02:00:00+00:00");
        let conn = mem();
        born_on(&conn, "2026-11-01T02:00:00+00:00");
        // 11 points before today — one short of R1.
        db::upsert_progress(
            &conn,
            "2026-11-05",
            &db::DayEvidence { present: 1, hits: 1, score: 11, ..Default::default() },
        )
        .unwrap();
        let _c2 = clock::pinned("2026-11-10T02:00:00+00:00");
        let now = local(2026, 11, 10, 10);
        // Today: three hits (would be +2 if today counted).
        for _ in 0..3 {
            db::add_event(&conn, "dialogue", "hi").unwrap();
        }
        predict(&conn, "2026-11-10", "hit");
        let out = tick(&conn, now).unwrap();
        assert_eq!(out.stage, 0, "11 + today's 2 must not reach 12 today");
        let today = db::progress_on_day(&conn, "2026-11-10").unwrap().unwrap();
        assert!(today.score > 0, "the day is still recorded — just not counted");
    }

    /// Release gate ③ (plan §4): cold needs a relationship to cool down,
    /// derived events are not activity, and the stage survives.
    #[test]
    fn cold_only_after_promotion_and_derived_events_do_not_count() {
        let _c = clock::pinned("2026-11-01T02:00:00+00:00");
        let conn = mem();
        born_on(&conn, "2026-11-01T02:00:00+00:00");
        db::add_event(&conn, "dialogue", "hi").unwrap();

        // Day 2, stage 0: seven quiet days must NOT make a polite stranger
        // "rusty" — there is no relationship to cool.
        let _c2 = clock::pinned("2026-11-10T02:00:00+00:00");
        let out = tick(&conn, local(2026, 11, 10, 10)).unwrap();
        assert_eq!(out.stage, 0, "no evidence → still 拘谨");
        assert_eq!(out.cold_changed, None);
        assert!(!status_payload(&conn).cold);

        // Promote by hand, then go quiet 8 days.
        db::set_relationship_stage(&conn, 1).unwrap();
        let _c3 = clock::pinned("2026-11-20T02:00:00+00:00");
        // Our own derived writes must not keep it warm.
        db::add_event(&conn, "relationship_stage", "{\"stage\":1}").unwrap();
        db::add_event(&conn, "disagreement", "我不同意").unwrap();
        let out2 = tick(&conn, local(2026, 11, 20, 10)).unwrap();
        assert_eq!(out2.cold_changed, Some(true), "derived events are not activity");
        assert_eq!(out2.stage, 1, "cold never rewinds the stage");

        // One real interaction thaws it on the same tick.
        let _c4 = clock::pinned("2026-11-20T03:00:00+00:00");
        db::add_event(&conn, "dialogue", "我回来了").unwrap();
        let out3 = tick(&conn, local(2026, 11, 20, 11)).unwrap();
        assert_eq!(out3.cold_changed, Some(false));
        assert_eq!(out3.stage, 1);
    }

    #[test]
    fn old_user_backfill_announces_once_with_a_look_back_tone() {
        let _c = clock::pinned("2026-11-01T02:00:00+00:00");
        let conn = mem();
        born_on(&conn, "2026-11-01T02:00:00+00:00");
        // 25 days of strong evidence, backfilled.
        for d in 1..=25 {
            let day = if d <= 19 {
                format!("2026-11-{:02}", d)
            } else {
                format!("2026-11-{d}")
            };
            if d > 19 {
                continue;
            }
            db::upsert_progress(
                &conn,
                &day,
                &db::DayEvidence { present: 1, hits: 1, score: 2, ..Default::default() },
            )
            .unwrap();
        }
        let _c2 = clock::pinned("2026-11-25T02:00:00+00:00");
        let out = tick(&conn, local(2026, 11, 25, 10)).unwrap();
        assert_eq!(out.stage, 2, "day 24 with 38 evidence jumps straight to R2");
        let pending = db::pending_relationship_logs(&conn).unwrap();
        assert_eq!(pending.len(), 1, "one announcement, not two");
        assert!(pending[0].3.contains("处了一阵子"), "look-back tone: {}", pending[0].3);
    }

    #[test]
    fn first_refusal_fires_once_inside_the_window_only() {
        let _c = clock::pinned("2026-11-01T02:00:00+00:00");
        let conn = mem();
        born_on(&conn, "2026-10-01T02:00:00+00:00");
        db::add_event(&conn, "dialogue", "hi").unwrap();
        let _c2 = clock::pinned("2026-11-01T03:00:00+00:00");
        // The row must exist first — an UPDATE against no row is a silent
        // no-op (the same trap the form-line poke test hit).
        db::ensure_relationship(&conn).unwrap();
        db::set_relationship_stage(&conn, 2).unwrap(); // stage_since = 11-01 03:00

        let now = local(2026, 11, 3, 10);
        // Stage 2, inside the window, no shut_up → fires.
        assert!(maybe_first_refusal(&conn, now).unwrap());
        // Second time → already fired.
        assert!(!maybe_first_refusal(&conn, now).unwrap());

        // Stage 1 never qualifies.
        let conn3 = mem();
        born_on(&conn3, "2026-10-01T02:00:00+00:00");
        db::add_event(&conn3, "dialogue", "hi").unwrap();
        db::ensure_relationship(&conn3).unwrap();
        db::set_relationship_stage(&conn3, 1).unwrap();
        assert!(!maybe_first_refusal(&conn3, local(2026, 11, 2, 10)).unwrap());

        // Outside the 7-day window it can never fire (fresh state, no log).
        let conn2 = mem();
        born_on(&conn2, "2026-10-01T02:00:00+00:00");
        db::add_event(&conn2, "dialogue", "hi").unwrap();
        db::ensure_relationship(&conn2).unwrap();
        db::set_relationship_stage(&conn2, 2).unwrap();
        let _c3 = clock::pinned("2026-11-20T03:00:00+00:00");
        assert!(!maybe_first_refusal(&conn2, local(2026, 11, 20, 10)).unwrap());
    }

    #[test]
    fn backoff_rate_is_none_without_asks_and_parses_recent_window() {
        let conn = mem();
        let now = local(2026, 11, 19, 10);
        assert_eq!(backoff_rate_recent(&conn, now, 7).unwrap(), None);
        predict(&conn, "2026-11-18", "shut_up");
        predict(&conn, "2026-11-17", "hit");
        let r = backoff_rate_recent(&conn, now, 7).unwrap().unwrap();
        assert!((r - 0.5).abs() < 1e-9);
        // Far in the past → outside the window → None again.
        assert_eq!(backoff_rate_recent(&conn, local(2026, 12, 19, 10), 7).unwrap(), None);
    }
}

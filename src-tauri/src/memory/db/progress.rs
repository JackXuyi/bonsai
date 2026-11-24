//! Predict-verify loop, form-line growth, relationship (plan-predict / morph / relationship).

use chrono::Timelike;
use rusqlite::{params, Connection};

use crate::memory::clock;

// ------------------------------------------------------- predict-verify loop
// (docs/zh/plan-predict.md §3.4, §4)

#[derive(Debug, Clone)]
pub struct PredictionRow {
    pub id: i64,
    pub date: String,
    pub guess: String,
    pub basis: String,
    pub ask_after_hour: i64,
    pub asked_at: Option<String>,
    pub skip_reason: Option<String>,
    pub feedback: Option<String>,
    pub feedback_at: Option<String>,
    pub resolved_at: Option<String>,
}

const PREDICTION_COLS: &str =
    "id, date, guess, basis, ask_after_hour, asked_at, skip_reason, feedback, feedback_at, resolved_at";

fn row_prediction(row: &rusqlite::Row) -> rusqlite::Result<PredictionRow> {
    Ok(PredictionRow {
        id: row.get(0)?,
        date: row.get(1)?,
        guess: row.get(2)?,
        basis: row.get(3)?,
        ask_after_hour: row.get(4)?,
        asked_at: row.get(5)?,
        skip_reason: row.get(6)?,
        feedback: row.get(7)?,
        feedback_at: row.get(8)?,
        resolved_at: row.get(9)?,
    })
}

/// Today's row, if one exists (asked or skipped).
pub fn prediction_on_day(conn: &Connection, date: &str) -> rusqlite::Result<Option<PredictionRow>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {PREDICTION_COLS} FROM predictions WHERE date = ?1"
    ))?;
    let mut rows = stmt.query_map(params![date], row_prediction)?;
    rows.next().transpose()
}

/// Fetch a prediction row by id (feedback writeback needs the basis).
pub fn prediction_on_day_by_id(
    conn: &Connection,
    id: i64,
) -> rusqlite::Result<PredictionRow> {
    conn.query_row(
        &format!("SELECT {PREDICTION_COLS} FROM predictions WHERE id = ?1"),
        params![id],
        row_prediction,
    )
}

/// Insert today's ASKED prediction. `UNIQUE(date)` makes double-ask
/// impossible; callers resolve the existing row first.
pub fn record_prediction(
    conn: &Connection,
    date: &str,
    guess: &str,
    basis_json: &str,
    ask_after_hour: i64,
) -> rusqlite::Result<i64> {
    conn.execute(
        "INSERT INTO predictions (date, guess, basis, ask_after_hour, asked_at)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(date) DO UPDATE SET
           guess = excluded.guess, basis = excluded.basis,
           ask_after_hour = excluded.ask_after_hour, asked_at = excluded.asked_at
         WHERE asked_at IS NULL",
        params![date, guess, basis_json, ask_after_hour, clock::now_iso()],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Record a skip decision for today (overwrites previous skip reason while
/// nothing has been asked — e.g. busy earlier, no_candidate later).
pub fn record_skip(conn: &Connection, date: &str, reason: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO predictions (date, guess, basis, ask_after_hour, skip_reason)
         VALUES (?1, '', '[]', 0, ?2)
         ON CONFLICT(date) DO UPDATE SET skip_reason = excluded.skip_reason
         WHERE asked_at IS NULL",
        params![date, reason],
    )?;
    Ok(())
}

/// Apply user feedback. Fails (Ok(false)) when already resolved or the 48h
/// window has closed — late feedback doesn't rewrite history.
pub fn set_prediction_feedback(
    conn: &Connection,
    id: i64,
    feedback: &str,
) -> rusqlite::Result<bool> {
    let (asked_at, existing_feedback, resolved_at): (
        Option<String>,
        Option<String>,
        Option<String>,
    ) = conn.query_row(
        "SELECT asked_at, feedback, resolved_at FROM predictions WHERE id = ?1",
        params![id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    let Some(asked_at) = asked_at else {
        return Ok(false); // never asked
    };
    if existing_feedback.is_some() || resolved_at.is_some() {
        return Ok(false); // already has feedback or resolved
    }
    let Some(asked) = chrono::DateTime::parse_from_rfc3339(&asked_at).ok() else {
        return Ok(false);
    };
    // "now" goes through the injectable clock so the 48h window is
    // deterministic under tests.
    let now = chrono::DateTime::parse_from_rfc3339(&clock::now_iso())
        .map(|t| t.with_timezone(&chrono::Utc))
        .unwrap_or_else(|_| chrono::Utc::now());
    if now - asked.with_timezone(&chrono::Utc) > chrono::Duration::hours(48) {
        return Ok(false);
    }
    let now_iso = clock::now_iso();
    let n = conn.execute(
        "UPDATE predictions SET feedback = ?2, feedback_at = ?3, resolved_at = ?3 WHERE id = ?1",
        params![id, feedback, now_iso],
    )?;
    Ok(n > 0)
}

/// Close the 48h window on stale unanswered predictions (weak negative
/// sample for the back-off-rate metric, plan §4). Called from the poll loop.
pub fn resolve_stale_predictions(conn: &Connection) -> rusqlite::Result<usize> {
    let now = chrono::DateTime::parse_from_rfc3339(&clock::now_iso())
        .map(|t| t.with_timezone(&chrono::Utc))
        .unwrap_or_else(|_| chrono::Utc::now());
    let cutoff = (now - chrono::Duration::hours(48)).to_rfc3339();
    conn.execute(
        "UPDATE predictions SET resolved_at = ?1
         WHERE asked_at IS NOT NULL AND feedback IS NULL AND resolved_at IS NULL
           AND asked_at < ?2",
        params![clock::now_iso(), cutoff],
    )
}

/// Active habits, highest confidence first — the predictor's evidence source.
pub fn active_habits(conn: &Connection) -> rusqlite::Result<Vec<(i64, String, f64)>> {
    let mut stmt = conn.prepare(
        "SELECT id, content, confidence FROM preferences
         WHERE category = 'habit' AND invalid_at IS NULL
         ORDER BY confidence DESC, id ASC",
    )?;
    let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
    rows.collect()
}

/// Quantized preference confidence bump (same two-decimal rule as facts;
/// clamped to [0.3, 0.95] — plan-predict.md §4).
pub fn bump_preference(conn: &Connection, pref_id: i64, delta: f64) -> rusqlite::Result<()> {
    let current: f64 = conn.query_row(
        "SELECT confidence FROM preferences WHERE id = ?1",
        params![pref_id],
        |r| r.get(0),
    )?;
    let next = super::quantize_conf((current + delta).clamp(0.3, 0.95));
    conn.execute(
        "UPDATE preferences SET confidence = ?1 WHERE id = ?2",
        params![next, pref_id],
    )?;
    Ok(())
}

/// One cell of the usage rhythm: how many distinct days did `app` show up
/// around `hour` (local time) over the look-back window, plus one sample
/// event id for basis traceability (plan-predict.md: explainable guesses).
#[derive(Debug, Clone)]
pub struct RhythmEntry {
    pub app: String,
    pub hour: u32,
    pub days: usize,
    pub sample_event_id: i64,
}

/// Days-per-(app, local-hour) histogram over the last `days` days, from
/// perception events of the shape "app: X". Feeds rule ② of RulePredictor.
pub fn app_rhythm(conn: &Connection, days: i64) -> rusqlite::Result<Vec<RhythmEntry>> {
    use std::collections::BTreeMap;
    let since = (chrono::Utc::now() - chrono::Duration::days(days)).to_rfc3339();
    let mut stmt = conn.prepare(
        "SELECT id, ts, content FROM events
         WHERE kind = 'perception' AND content LIKE 'app: %' AND ts >= ?1 ORDER BY ts ASC",
    )?;
    let rows = stmt.query_map(params![since], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?))
    })?;
    // (app, hour) -> distinct local dates; first-seen event id kept.
    let mut seen: BTreeMap<(String, u32), (std::collections::BTreeSet<String>, i64)> =
        BTreeMap::new();
    for row in rows {
        let (id, ts, content) = row?;
        let app = content.trim_start_matches("app: ").trim().to_string();
        let parsed = chrono::DateTime::parse_from_rfc3339(&ts)
            .map(|d| d.with_timezone(&chrono::Local));
        let (hour, date) = match parsed {
            Ok(local) => (local.hour(), local.format("%Y-%m-%d").to_string()),
            Err(_) => continue,
        };
        let entry = seen.entry((app, hour)).or_insert_with(|| (Default::default(), id));
        entry.0.insert(date);
    }
    Ok(seen
        .into_iter()
        .map(|((app, hour), (dates, sample))| RhythmEntry {
            app,
            hour,
            days: dates.len(),
            sample_event_id: sample,
        })
        .collect())
}

/// Did the proactive dialogue channel already speak today (mutual exclusion,
/// plan §3.3: a day that had its proactive message gets no prediction)?
pub fn proactive_spoke_today(conn: &Connection, day: chrono::NaiveDate) -> rusqlite::Result<bool> {
    // 「今天」必须由调用方传入（predict 传 schedule_iteration 的 now）：测试
    // 用 clock::pinned 钉住时间线时，真实 Local::now() 会查出空窗口导致误判。
    let (start, end) = crate::report::local_day_window(day).unwrap_or_default();
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM events
         WHERE role = 'assistant' AND session_id = 'proactive' AND ts >= ?1 AND ts < ?2",
        params![start, end],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// Aggregate stats over resolved predictions (plan §4 metrics).
#[derive(Debug, Default, serde::Serialize)]
pub struct PredictionStats {
    pub hit: usize,
    pub miss: usize,
    pub shut_up: usize,
    pub no_reply: usize,
    pub asked: usize,
    /// hit / (hit + miss) over ALL resolved-with-verdict rows.
    pub accuracy: Option<f64>,
    /// hit / (hit + miss) over the last 7 resolved days, `None` if no verdicts.
    pub accuracy_7d: Option<f64>,
    /// shut_up / asked, `None` when nothing asked.
    pub backoff_rate: Option<f64>,
}

pub fn prediction_stats(conn: &Connection) -> rusqlite::Result<PredictionStats> {
    let mut s = PredictionStats::default();
    let mut stmt = conn.prepare(
        "SELECT feedback, asked_at, resolved_at FROM predictions WHERE asked_at IS NOT NULL",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, Option<String>>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?,
        ))
    })?;
    let mut verdict_7d: (usize, usize) = (0, 0); // (hit, miss) within 7 days
    for row in rows {
        let (feedback, asked_at, resolved_at) = row?;
        s.asked += 1;
        match feedback.as_deref() {
            Some("hit") => s.hit += 1,
            Some("miss") => s.miss += 1,
            Some("shut_up") => s.shut_up += 1,
            _ => {}
        }
        if resolved_at.is_none() && feedback.is_none() {
            s.no_reply += 1;
        }
        let recent = chrono::DateTime::parse_from_rfc3339(&asked_at)
            .map(|t| chrono::Utc::now() - t.with_timezone(&chrono::Utc) <= chrono::Duration::days(7))
            .unwrap_or(false);
        if recent {
            match feedback.as_deref() {
                Some("hit") => verdict_7d.0 += 1,
                Some("miss") => verdict_7d.1 += 1,
                _ => {}
            }
        }
    }
    let total_verdicts = s.hit + s.miss;
    if total_verdicts > 0 {
        s.accuracy = Some(s.hit as f64 / total_verdicts as f64);
    }
    let v7 = verdict_7d.0 + verdict_7d.1;
    if v7 > 0 {
        s.accuracy_7d = Some(verdict_7d.0 as f64 / v7 as f64);
    }
    if s.asked > 0 {
        s.backoff_rate = Some(s.shut_up as f64 / s.asked as f64);
    }
    Ok(s)
}

// ------------------------------------------------- form-line growth
// (docs/zh/plan-morph.md §3.2)

#[derive(Debug, Clone)]
pub struct GrowthState {
    pub born_at: String,
    pub stage: i64,
    pub last_tick_date: Option<String>,
    pub dormant_since: Option<String>,
}

/// The single growth row, if it exists yet.
pub fn growth_state(conn: &Connection) -> rusqlite::Result<Option<GrowthState>> {
    let mut stmt = conn.prepare(
        "SELECT born_at, stage, last_tick_date, dormant_since FROM growth WHERE id = 1",
    )?;
    let mut rows = stmt.query_map([], |row| {
        Ok(GrowthState {
            born_at: row.get(0)?,
            stage: row.get(1)?,
            last_tick_date: row.get(2)?,
            dormant_since: row.get(3)?,
        })
    })?;
    rows.next().transpose()
}

/// Ensure the growth row exists, anchoring `born_at` to the earliest event
/// (plan open question 3: existing users don't restart the clock — "it has
/// actually been with you for N days" is the stronger first impression).
/// The anchor is written once and never changed afterwards.
pub fn ensure_growth(conn: &Connection) -> rusqlite::Result<GrowthState> {
    if let Some(s) = growth_state(conn)? {
        return Ok(s);
    }
    let earliest: Option<String> = conn
        .query_row("SELECT MIN(ts) FROM events", [], |r| r.get(0))
        .ok()
        .flatten();
    let born_at = earliest.unwrap_or_else(|| clock::now_iso());
    conn.execute(
        "INSERT INTO growth (id, born_at, stage) VALUES (1, ?1, 0)",
        params![born_at],
    )?;
    Ok(GrowthState {
        born_at,
        stage: 0,
        last_tick_date: None,
        dormant_since: None,
    })
}

pub fn set_growth_stage(conn: &Connection, stage: i64) -> rusqlite::Result<()> {
    // Guarded by `stage > ?2` at the call site; the WHERE re-asserts the
    // monotonicity invariant (stage only ever increases).
    conn.execute(
        "UPDATE growth SET stage = ?1 WHERE id = 1 AND stage < ?1",
        params![stage],
    )?;
    Ok(())
}

pub fn set_growth_tick_date(conn: &Connection, date: &str) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE growth SET last_tick_date = ?1 WHERE id = 1",
        params![date],
    )?;
    Ok(())
}

/// `Some(iso)` = went dormant; `None` = woke up. Never overwrites an
/// existing dormant_since with another one (first sleep wins until wake).
pub fn set_dormant(conn: &Connection, since: Option<&str>) -> rusqlite::Result<usize> {
    match since {
        Some(iso) => conn.execute(
            "UPDATE growth SET dormant_since = ?1 WHERE id = 1 AND dormant_since IS NULL",
            params![iso],
        ),
        None => conn.execute(
            "UPDATE growth SET dormant_since = NULL WHERE id = 1 AND dormant_since IS NOT NULL",
            [],
        ),
    }
}

pub fn add_growth_log(conn: &Connection, stage: i64, note: &str) -> rusqlite::Result<i64> {
    conn.execute(
        "INSERT INTO growth_log (stage, unlocked_at, note) VALUES (?1, ?2, ?3)",
        params![stage, clock::now_iso(), note],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Un-announced unlocks (oldest first) — the window-open announcement queue.
pub fn pending_growth_logs(conn: &Connection) -> rusqlite::Result<Vec<(i64, i64, String)>> {
    let mut stmt = conn.prepare(
        "SELECT id, stage, note FROM growth_log
         WHERE notified_at IS NULL ORDER BY id ASC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?, row.get::<_, String>(2)?))
    })?;
    rows.collect()
}

/// Idempotent announcement: only a row with notified_at still NULL is
/// marked, so a double window-open cannot announce twice.
pub fn mark_growth_notified(conn: &Connection, id: i64) -> rusqlite::Result<bool> {
    let n = conn.execute(
        "UPDATE growth_log SET notified_at = ?1 WHERE id = ?2 AND notified_at IS NULL",
        params![clock::now_iso(), id],
    )?;
    Ok(n > 0)
}

// ------------------------------------------------- relationship (plan §3.3)

#[derive(Debug, Clone)]
pub struct RelationshipState {
    pub stage: i64,
    pub stage_since: Option<String>,
    pub cold_since: Option<String>,
    pub last_tick_date: Option<String>,
}

/// One day of understanding evidence (plan §3.2). `score` is derived by
/// `relationship::score_day` — it exists here only as an audit column and is
/// NEVER returned to the frontend.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DayEvidence {
    pub present: i64,
    pub hits: i64,
    pub misses: i64,
    pub disagreements: i64,
    pub shut_ups: i64,
    pub score: i64,
}

pub fn relationship_state(conn: &Connection) -> rusqlite::Result<Option<RelationshipState>> {
    let mut stmt = conn.prepare(
        "SELECT stage, stage_since, cold_since, last_tick_date FROM relationship WHERE id = 1",
    )?;
    let mut rows = stmt.query_map([], |row| {
        Ok(RelationshipState {
            stage: row.get(0)?,
            stage_since: row.get(1)?,
            cold_since: row.get(2)?,
            last_tick_date: row.get(3)?,
        })
    })?;
    rows.next().transpose()
}

/// Ensure the single row exists. Unlike `ensure_growth` there is no anchor to
/// backfill — the relationship timeline reuses `growth.born_at`.
pub fn ensure_relationship(conn: &Connection) -> rusqlite::Result<RelationshipState> {
    if let Some(s) = relationship_state(conn)? {
        return Ok(s);
    }
    conn.execute("INSERT OR IGNORE INTO relationship (id, stage) VALUES (1, 0)", [])?;
    Ok(RelationshipState {
        stage: 0,
        stage_since: None,
        cold_since: None,
        last_tick_date: None,
    })
}

/// Monotonic stage write (the WHERE re-asserts invariant 1: never rewinds).
pub fn set_relationship_stage(conn: &Connection, stage: i64) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE relationship SET stage = ?1, stage_since = ?2 WHERE id = 1 AND stage < ?1",
        params![stage, clock::now_iso()],
    )?;
    Ok(())
}

/// `Some(iso)` = went cold; `None` = warmed up. Same double-write guard as
/// `set_dormant`: the first cold stamp wins until it actually thaws.
pub fn set_relationship_cold(conn: &Connection, since: Option<&str>) -> rusqlite::Result<usize> {
    match since {
        Some(iso) => conn.execute(
            "UPDATE relationship SET cold_since = ?1 WHERE id = 1 AND cold_since IS NULL",
            params![iso],
        ),
        None => conn.execute(
            "UPDATE relationship SET cold_since = NULL WHERE id = 1 AND cold_since IS NOT NULL",
            [],
        ),
    }
}

pub fn set_relationship_tick_date(conn: &Connection, date: &str) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE relationship SET last_tick_date = ?1 WHERE id = 1",
        params![date],
    )?;
    Ok(())
}

/// Recompute-then-store for one local day. Idempotent by construction: the
/// caller derives `ev` from source tables every time, so a second call with
/// unchanged sources writes the same numbers.
pub fn upsert_progress(
    conn: &Connection,
    date: &str,
    ev: &DayEvidence,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO relationship_progress
             (date, present, hits, misses, disagreements, shut_ups, score, computed_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(date) DO UPDATE SET
             present = excluded.present,
             hits = excluded.hits,
             misses = excluded.misses,
             disagreements = excluded.disagreements,
             shut_ups = excluded.shut_ups,
             score = excluded.score,
             computed_at = excluded.computed_at",
        params![
            date,
            ev.present,
            ev.hits,
            ev.misses,
            ev.disagreements,
            ev.shut_ups,
            ev.score,
            clock::now_iso()
        ],
    )?;
    Ok(())
}

pub fn progress_on_day(conn: &Connection, date: &str) -> rusqlite::Result<Option<DayEvidence>> {
    let mut stmt = conn.prepare(
        "SELECT present, hits, misses, disagreements, shut_ups, score
         FROM relationship_progress WHERE date = ?1",
    )?;
    let mut rows = stmt.query_map(params![date], |row| {
        Ok(DayEvidence {
            present: row.get(0)?,
            hits: row.get(1)?,
            misses: row.get(2)?,
            disagreements: row.get(3)?,
            shut_ups: row.get(4)?,
            score: row.get(5)?,
        })
    })?;
    rows.next().transpose()
}

/// Cumulative evidence BEFORE `date` — today never counts, so a single hit
/// cannot promote the relationship mid-day (plan §3.3).
pub fn progress_sum_before(conn: &Connection, date: &str) -> rusqlite::Result<i64> {
    conn.query_row(
        "SELECT COALESCE(SUM(score), 0) FROM relationship_progress WHERE date < ?1",
        params![date],
        |r| r.get(0),
    )
}

/// Cumulative evidence in the half-open window `[from, before)` — the rolling
/// 30-day view used by the promotion rule (plan §3.3).
pub fn progress_sum_between(
    conn: &Connection,
    from: &str,
    before: &str,
) -> rusqlite::Result<i64> {
    conn.query_row(
        "SELECT COALESCE(SUM(score), 0) FROM relationship_progress
         WHERE date >= ?1 AND date < ?2",
        params![from, before],
        |r| r.get(0),
    )
}

pub fn add_relationship_log(conn: &Connection, stage: i64, note: &str) -> rusqlite::Result<i64> {
    add_relationship_log_kind(conn, "stage", stage, note)
}

pub fn add_relationship_log_kind(
    conn: &Connection,
    kind: &str,
    stage: i64,
    note: &str,
) -> rusqlite::Result<i64> {
    conn.execute(
        "INSERT INTO relationship_log (kind, stage, unlocked_at, note) VALUES (?1, ?2, ?3, ?4)",
        params![kind, stage, clock::now_iso(), note],
    )?;
    Ok(conn.last_insert_rowid())
}

/// (id, kind, stage, note) — oldest first, un-announced only.
pub fn pending_relationship_logs(
    conn: &Connection,
) -> rusqlite::Result<Vec<(i64, String, i64, String)>> {
    let mut stmt = conn.prepare(
        "SELECT id, kind, stage, note FROM relationship_log
         WHERE notified_at IS NULL ORDER BY id ASC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, String>(3)?,
        ))
    })?;
    rows.collect()
}

pub fn mark_relationship_notified(conn: &Connection, id: i64) -> rusqlite::Result<bool> {
    let n = conn.execute(
        "UPDATE relationship_log SET notified_at = ?1 WHERE id = ?2 AND notified_at IS NULL",
        params![clock::now_iso(), id],
    )?;
    Ok(n > 0)
}


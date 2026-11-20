//! P1 item 1 — the predict-verify loop (docs/zh/plan-predict.md).
//!
//! Once a day the companion guesses what the user is about to do, based on
//! the memory layer's habits + the perception rhythm. The user answers with
//! one of three verdicts: hit / miss / shut_up (silence after 48h counts as
//! "no reply"). Feedback flows back into preference confidence; streaks feed
//! the (P2) relationship line.
//!
//! Invariants (plan §3.1):
//! - 宁缺毋滥: no evidence → no prediction, the day is recorded as skipped.
//! - Explainable: every candidate carries a non-empty `basis` (preference or
//!   event ids). No basis, no guess.
//! - One per day: UNIQUE(date) in the schema; mutual exclusion with the
//!   proactive chat channel — a day that already had a proactive message
//!   gets no prediction (at most one intrusion per day, report §6.4).
//! - Predictions are never rewritten: feedback is appended, late feedback
//!   after the 48h window is rejected.

use chrono::{Datelike, Timelike};
use rusqlite::Connection;
use serde::Serialize;

use crate::memory::db;

/// Longest back-off after a "别烦我" (fixed for P1, plan open question 1).
pub const BACKOFF_DAYS: i64 = 3;
/// Delivery window tolerance around `ask_after_hour` (local time).
pub const WINDOW_TOLERANCE_HOURS: u32 = 1;
/// Days of perception history the rhythm rule looks at.
pub const RHYTHM_DAYS: i64 = 14;
/// Minimum distinct days for a rhythm entry to count as a pattern.
pub const RHYTHM_MIN_DAYS: usize = 5;

// ---------------------------------------------------------------- candidate

/// Evidence trace: which row produced this guess (explainability invariant).
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Basis {
    Preference { id: i64 },
    Event { id: i64 },
}

#[derive(Debug, Clone)]
pub struct PredictionCandidate {
    pub guess: String,
    pub basis: Vec<Basis>,
    /// Intended delivery hour (local). Delivery happens within ±1h.
    pub ask_after_hour: u32,
    pub confidence: f64,
}

impl PredictionCandidate {
    pub fn basis_json(&self) -> String {
        serde_json::to_string(&self.basis).unwrap_or_else(|_| "[]".into())
    }
}

/// Evidence digest handed to a generator (mirrors SessionDigest in extract.rs).
#[derive(Debug, Default)]
pub struct PredictionDigest {
    /// Active habits, highest confidence first.
    pub habits: Vec<(i64, String, f64)>,
    /// 14-day (app, local-hour) → distinct-days rhythm histogram.
    pub rhythm: Vec<db::RhythmEntry>,
    /// Apps seen in perception yesterday, earliest local hour first,
    /// with the event id of the first sighting (basis traceability).
    pub yesterday_apps: Vec<(String, u32, i64)>,
    /// Apps already seen in today's perception (rule ②/③ suppression).
    pub today_apps: std::collections::BTreeSet<String>,
}

pub fn build_digest(conn: &Connection) -> rusqlite::Result<PredictionDigest> {
    // Yesterday's app events, deduped by app keeping the earliest hour + id.
    let mut yesterday_apps: Vec<(String, u32, i64)> = Vec::new();
    let mut today_apps: std::collections::BTreeSet<String> = Default::default();
    {
        let day = chrono::Local::now().date_naive() - chrono::Duration::days(1);
        if let Some((ws, we)) = crate::report::local_day_window(day) {
            use std::collections::BTreeMap;
            let mut first: BTreeMap<String, (i64, u32)> = BTreeMap::new();
            for row in query_app_events(conn, &ws, &we)? {
                let (id, ts, content) = row;
                let app = content.trim_start_matches("app: ").trim().to_string();
                if let Ok(local) = chrono::DateTime::parse_from_rfc3339(&ts) {
                    let hour = local.with_timezone(&chrono::Local).hour();
                    first.entry(app).or_insert((id, hour));
                }
            }
            yesterday_apps = first.into_iter().map(|(a, (i, h))| (a, h, i)).collect();
        }
    }
    {
        let day = chrono::Local::now().date_naive();
        if let Some((ws, we)) = crate::report::local_day_window(day) {
            for (_, _, content) in query_app_events(conn, &ws, &we)? {
                today_apps.insert(content.trim_start_matches("app: ").trim().to_string());
            }
        }
    }
    Ok(PredictionDigest {
        habits: db::active_habits(conn)?,
        rhythm: db::app_rhythm(conn, RHYTHM_DAYS)?,
        yesterday_apps,
        today_apps,
    })
}

/// Shared query for "app: X" perception events inside a UTC window.
fn query_app_events(
    conn: &Connection,
    ws: &str,
    we: &str,
) -> rusqlite::Result<Vec<(i64, String, String)>> {
    let mut stmt = conn.prepare(
        "SELECT id, ts, content FROM events
         WHERE kind = 'perception' AND content LIKE 'app: %' AND ts >= ?1 AND ts < ?2
         ORDER BY ts ASC",
    )?;
    let rows = stmt.query_map(rusqlite::params![ws, we], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    rows.collect()
}

// ---------------------------------------------------------------- generator

pub trait PredictionGenerator {
    fn predict(&self, digest: &PredictionDigest) -> Option<PredictionCandidate>;
}

/// Deterministic hour extraction from habit text like "用户 14 点左右开会".
/// First 0–23 number before 点 (whitespace tolerated); None if absent.
fn habit_hour(content: &str) -> Option<u32> {
    let chars: Vec<char> = content.chars().collect();
    for (i, c) in chars.iter().enumerate() {
        if *c == '点' && i > 0 {
            let mut digits = String::new();
            let mut j = i;
            // Skip whitespace between the number and 点.
            while j > 0 && chars[j - 1].is_whitespace() {
                j -= 1;
            }
            while j > 0 && chars[j - 1].is_ascii_digit() && digits.len() < 2 {
                j -= 1;
                digits.insert(0, chars[j]);
            }
            if let Ok(h) = digits.parse::<u32>() {
                if h <= 23 {
                    return Some(h);
                }
            }
        }
    }
    None
}

/// Three tone templates, cycled by day-of-year so the phrasing doesn't get
/// stale (plan open question 3). P1 keeps these deterministic; the model
/// phrasing leg is LocalLmPredictor (G5).
fn tone(guess_core: &str) -> String {
    let variant = chrono::Local::now().ordinal() % 3;
    match variant {
        0 => format!("我猜你接下来大概要{guess_core}——猜错了别笑我。"),
        1 => format!("按我的观察，{guess_core}的时间差不多了？对不对你说个准话。"),
        _ => format!("看你最近的样子，{guess_core}吧？我这次可留意着呢。"),
    }
}

pub struct RulePredictor;

impl PredictionGenerator for RulePredictor {
    /// Rule priority (plan §3.2): habit direct > rhythm extrapolation >
    /// continuity. Highest-confidence candidate wins on ties within a rule;
    /// between rules, earlier rules win outright.
    fn predict(&self, digest: &PredictionDigest) -> Option<PredictionCandidate> {
        // ① habit direct: top habit, hour parsed from its text (default 10).
        if let Some((id, content, conf)) = digest.habits.first() {
            let hour = habit_hour(content).unwrap_or(10);
            let core = content
                .trim_start_matches(|c: char| "用户我你".contains(c))
                .trim()
                .to_string();
            return Some(PredictionCandidate {
                guess: tone(&core),
                basis: vec![Basis::Preference { id: *id }],
                ask_after_hour: hour,
                confidence: *conf,
            });
        }

        // ② rhythm extrapolation: an app that appeared on ≥5 of the last 14
        // days around this hour, and hasn't shown up today.
        let now_hour = chrono::Local::now().hour();
        let mut rhythm_best: Option<(f64, &db::RhythmEntry)> = None;
        for entry in &digest.rhythm {
            if entry.days < RHYTHM_MIN_DAYS {
                continue;
            }
            let hdiff = hour_diff(entry.hour, now_hour);
            if hdiff > WINDOW_TOLERANCE_HOURS as i32 {
                continue;
            }
            if digest.today_apps.contains(&entry.app) {
                continue;
            }
            let score = entry.days as f64 - hdiff as f64 * 0.1;
            if rhythm_best.map(|(s, _)| score > s).unwrap_or(true) {
                rhythm_best = Some((score, entry));
            }
        }
        if let Some((_, entry)) = rhythm_best {
            return Some(PredictionCandidate {
                guess: tone(&format!("打开 {}", entry.app)),
                basis: vec![Basis::Event {
                    id: entry.sample_event_id,
                }],
                ask_after_hour: entry.hour,
                confidence: (0.4 + 0.05 * entry.days as f64).min(0.7),
            });
        }

        // ③ continuity: yesterday's earliest app, not yet seen today, at this
        // hour or later — "平时这个时候你已经在用 X 了" (a guess that may be
        // disliked is a feature, not a bug — plan §3.2).
        let mut cont_best: Option<(u32, String, u32, i64)> = None;
        for (app, hour, event_id) in &digest.yesterday_apps {
            if digest.today_apps.contains(app) {
                continue;
            }
            let hdiff = hour_diff(*hour, now_hour);
            if hdiff > 2 {
                continue;
            }
            if cont_best.as_ref().map(|(h, _, _, _)| *hour < *h).unwrap_or(true) {
                cont_best = Some((*hour, app.clone(), *hour, *event_id));
            }
        }
        if let Some((hour, app, _, event_id)) = cont_best {
            return Some(PredictionCandidate {
                guess: tone(&format!("开 {app} 干活")),
                basis: vec![Basis::Event { id: event_id }],
                ask_after_hour: hour,
                confidence: 0.45,
            });
        }
        None
    }
}

/// Hours between two wall-clock hours, wrapping past midnight (|23−1| = 2).
fn hour_diff(a: u32, b: u32) -> i32 {
    let d = (a as i32 - b as i32).abs();
    d.min(24 - d)
}

// ---------------------------------------------------------------- scheduling

#[derive(Debug, PartialEq)]
enum ScheduleOutcome {
    /// A prediction was delivered today.
    Asked(i64, String),
    /// Nothing today; reason recorded (or retried later without a record).
    Skipped(Option<&'static str>),
}

/// One scheduling iteration. Separated from the thread for testability.
/// Order of gates (plan §3.3): backoff → already-asked → candidate →
/// busy/idle → mutual exclusion → deliver.
fn schedule_iteration(
    conn: &Connection,
    digest: &PredictionDigest,
    now: chrono::DateTime<chrono::Local>,
    busy: bool,
) -> rusqlite::Result<ScheduleOutcome> {
    let date = now.format("%Y-%m-%d").to_string();

    // Back-off check: any shut_up within the last BACKOFF_DAYS silences
    // predictions entirely.
    let backoff_until = last_shut_up(conn)?.map(|d| d + chrono::Duration::days(BACKOFF_DAYS));
    if let Some(until) = backoff_until {
        // Block the BACKOFF_DAYS calendar days after the shut-up day itself.
        if now.date_naive() <= until {
            db::record_skip(conn, &date, "backoff")?;
            return Ok(ScheduleOutcome::Skipped(Some("backoff")));
        }
    }

    // Already asked today → done (skips are retryable within the day).
    if let Some(row) = db::prediction_on_day(conn, &date)? {
        if row.asked_at.is_some() {
            return Ok(ScheduleOutcome::Skipped(None));
        }
    }

    // Evidence or silence.
    let Some(candidate) = (RulePredictor).predict(digest) else {
        db::record_skip(conn, &date, "no_candidate")?;
        return Ok(ScheduleOutcome::Skipped(Some("no_candidate")));
    };

    // Inside the delivery window? (±1h around ask_after_hour)
    let hdiff = hour_diff(candidate.ask_after_hour, now.hour());
    if hdiff > WINDOW_TOLERANCE_HOURS as i32 {
        return Ok(ScheduleOutcome::Skipped(None)); // too early/late — wait, no record
    }

    // L4 gates: busy never interrupts; mutual exclusion with proactive chat.
    if busy {
        db::record_skip(conn, &date, "busy")?;
        return Ok(ScheduleOutcome::Skipped(Some("busy")));
    }
    if db::proactive_spoke_today(conn, now.date_naive())? {
        db::record_skip(conn, &date, "spoke_today")?;
        return Ok(ScheduleOutcome::Skipped(Some("spoke_today")));
    }

    let id = db::record_prediction(
        conn,
        &date,
        &candidate.guess,
        &candidate.basis_json(),
        candidate.ask_after_hour as i64,
    )?;
    Ok(ScheduleOutcome::Asked(id, candidate.guess))
}

/// Date of the most recent shut_up verdict, if any.
fn last_shut_up(conn: &Connection) -> rusqlite::Result<Option<chrono::NaiveDate>> {
    let row: Option<String> = conn
        .query_row(
            "SELECT MAX(feedback_at) FROM predictions WHERE feedback = 'shut_up'",
            [],
            |r| r.get(0),
        )
        .ok();
    Ok(row.and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok())
        .map(|t| t.with_timezone(&chrono::Local).date_naive()))
}

/// Poll thread: every minute, one scheduling iteration. Also closes stale
/// 48h feedback windows. Delivery emits `prediction:asked` for the frontend.
pub fn spawn(app: tauri::AppHandle) {
    use tauri::Manager;
    std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_secs(60));
        let Some(store) = app.try_state::<crate::memory::MemoryStore>() else {
            continue;
        };
        let Ok(conn) = store.inner().0.lock() else {
            continue;
        };
        let _ = db::resolve_stale_predictions(&conn);

        let busy = {
            // L1 signals: typing / fullscreen / meeting via the decision
            // layer's view of the latest signals.
            let signals = app
                .try_state::<crate::perception::PerceptionState>()
                .map(|p| {
                    p.inner()
                        .latest
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .clone()
                });
            match signals {
                Some(s) => {
                    // Same view as L4: a meeting in progress is "busy" for the
                    // guess channel too (plan-calendar.md §3.7).
                    let input = crate::perception::decision_input(
                        &s,
                        0,
                        crate::calendar::in_meeting_now(&conn, chrono::Local::now()),
                    );
                    input.is_typing || input.is_fullscreen || input.in_meeting
                }
                None => true, // unknown → don't interrupt
            }
        };

        let digest = match build_digest(&conn) {
            Ok(d) => d,
            Err(_) => continue,
        };
        let now = chrono::Local::now();
        match schedule_iteration(&conn, &digest, now, busy) {
            Ok(ScheduleOutcome::Asked(id, guess)) => {
                let _ = db::add_event(
                    &conn,
                    "prediction_asked",
                    &format!("{{\"id\":{id},\"guess\":{}}}", serde_json::to_string(&guess).unwrap_or_default()),
                );
                drop(conn);
                use tauri::Emitter;
                let _ = app.emit(
                    "prediction:asked",
                    serde_json::json!({ "id": id, "guess": guess }),
                );
            }
            _ => {}
        }
    });
}

// ---------------------------------------------------------------- feedback

/// User verdict on today's prediction: hit / miss / shut_up. Feedback flows
/// back into the memory layer (plan §4); late feedback (>48h) is rejected.
/// Separated from the command for testability.
pub fn apply_feedback(conn: &Connection, id: i64, verdict: &str) -> rusqlite::Result<bool> {
    let applied = db::set_prediction_feedback(conn, id, verdict)?;
    if applied {
        // Memory writeback: preference evidence ±, plan §4 table.
        if let Ok(row) = db::prediction_on_day_by_id(conn, id) {
            let basis: Vec<Basis> = serde_json::from_str(&row.basis).unwrap_or_default();
            let delta = match verdict {
                "hit" => 0.1,
                "miss" => -0.1,
                _ => -0.15,
            };
            for b in &basis {
                if let Basis::Preference { id } = b {
                    let _ = db::bump_preference(conn, *id, delta);
                }
            }
            let _ = db::add_event(
                conn,
                "prediction_feedback",
                &format!("{{\"prediction_id\":{id},\"verdict\":\"{verdict}\"}}"),
            );
        }
    }
    Ok(applied)
}

// ---------------------------------------------------------------- commands

/// User verdict on today's prediction: hit / miss / shut_up. Feedback flows
/// back into the memory layer (plan §4); late feedback (>48h) is rejected.
#[tauri::command]
pub fn prediction_feedback(
    store: tauri::State<'_, crate::memory::MemoryStore>,
    id: i64,
    verdict: String,
) -> Result<bool, String> {
    if !matches!(verdict.as_str(), "hit" | "miss" | "shut_up") {
        return Err("verdict must be hit | miss | shut_up".into());
    }
    let conn = store.inner().0.lock().map_err(|e| e.to_string())?;
    apply_feedback(&conn, id, &verdict).map_err(|e| e.to_string())
}

/// Aggregate metrics for the tools tab (plan §4).
#[tauri::command]
pub fn prediction_stats(
    store: tauri::State<'_, crate::memory::MemoryStore>,
) -> Result<db::PredictionStats, String> {
    let conn = store.inner().0.lock().map_err(|e| e.to_string())?;
    db::prediction_stats(&conn).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::clock;

    fn mem() -> Connection {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir = std::env::temp_dir().join("bonsai-predict-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!(
            "test-{}-{n}.sqlite3",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        db::open(&path).unwrap()
    }

    fn habit(conn: &Connection, content: &str, conf: f64) -> i64 {
        db::add_preference(conn, "habit", content, conf).unwrap()
    }

    fn local(y: i32, m: u32, d: u32, h: u32) -> chrono::DateTime<chrono::Local> {
        use chrono::TimeZone;
        chrono::Local
            .with_ymd_and_hms(y, m, d, h, 0, 0)
            .single()
            .unwrap()
    }

    #[test]
    fn habit_rule_takes_priority_and_parses_hour() {
        let digest = PredictionDigest {
            habits: vec![(1, "用户 14 点左右开会".into(), 0.7)],
            ..Default::default()
        };
        let c = RulePredictor.predict(&digest).unwrap();
        assert_eq!(c.ask_after_hour, 14);
        assert_eq!(c.basis.len(), 1);
        assert!(c.guess.contains("开会"));
    }

    #[test]
    fn habit_without_parseable_hour_defaults_to_ten() {
        let digest = PredictionDigest {
            habits: vec![(2, "用户喜欢在写完代码后散步".into(), 0.6)],
            ..Default::default()
        };
        let c = RulePredictor.predict(&digest).unwrap();
        assert_eq!(c.ask_after_hour, 10);
    }

    #[test]
    fn no_evidence_means_no_guess() {
        let c = RulePredictor.predict(&PredictionDigest::default());
        assert!(c.is_none(), "宁缺毋滥: empty digest → None");
    }

    #[test]
    fn schedule_no_candidate_records_skip_and_can_upgrade_later() {
        let _clock = clock::pinned("2026-11-19T02:00:00+00:00");
        let conn = mem();
        let digest = PredictionDigest::default();
        let now = local(2026, 11, 19, 10);
        match schedule_iteration(&conn, &digest, now, false).unwrap() {
            ScheduleOutcome::Skipped(Some("no_candidate")) => {}
            other => panic!("expected no_candidate skip, got {other:?}"),
        }
        let row = db::prediction_on_day(&conn, "2026-11-19").unwrap().unwrap();
        assert!(row.asked_at.is_none());
        assert_eq!(row.skip_reason.as_deref(), Some("no_candidate"));
    }

    #[test]
    fn schedule_outside_window_waits_without_recording() {
        let _clock = clock::pinned("2026-11-19T02:00:00+00:00");
        let conn = mem();
        let digest = PredictionDigest {
            habits: vec![(1, "用户 16 点左右散步".into(), 0.7)],
            ..Default::default()
        };
        let now = local(2026, 11, 19, 9); // far from 16
        match schedule_iteration(&conn, &digest, now, false).unwrap() {
            ScheduleOutcome::Skipped(None) => {}
            other => panic!("expected silent wait, got {other:?}"),
        }
        assert!(db::prediction_on_day(&conn, "2026-11-19").unwrap().is_none());
    }

    #[test]
    fn schedule_delivers_once_then_never_again() {
        let _clock = clock::pinned("2026-11-19T06:00:00+00:00");
        let conn = mem();
        let digest = PredictionDigest {
            habits: vec![(1, "用户 14 点左右开会".into(), 0.7)],
            ..Default::default()
        };
        let now = local(2026, 11, 19, 14);
        let id = match schedule_iteration(&conn, &digest, now, false).unwrap() {
            ScheduleOutcome::Asked(id, g) => (id, g),
            other => panic!("expected asked, got {other:?}"),
        };
        assert!(id.1.contains("开会"));
        // Same hour again → already asked.
        match schedule_iteration(&conn, &digest, now, false).unwrap() {
            ScheduleOutcome::Skipped(None) => {}
            other => panic!("expected quiet skip, got {other:?}"),
        }
    }

    #[test]
    fn busy_and_spoke_today_record_respective_skips() {
        let _clock = clock::pinned("2026-11-19T06:00:00+00:00");
        let conn = mem();
        let digest = PredictionDigest {
            habits: vec![(1, "用户 14 点左右开会".into(), 0.7)],
            ..Default::default()
        };
        let now = local(2026, 11, 19, 14);
        match schedule_iteration(&conn, &digest, now, true).unwrap() {
            ScheduleOutcome::Skipped(Some("busy")) => {}
            other => panic!("expected busy skip, got {other:?}"),
        }
        // Simulate the proactive channel having spoken today.
        db::add_event_meta(&conn, "dialogue", "早", Some("assistant"), Some(crate::dialogue::PROACTIVE_SESSION)).unwrap();
        match schedule_iteration(&conn, &digest, now, false).unwrap() {
            ScheduleOutcome::Skipped(Some("spoke_today")) => {}
            other => panic!("expected spoke_today skip, got {other:?}"),
        }
    }

    #[test]
    fn shut_up_backs_off_three_days() {
        let _clock = clock::pinned("2026-11-19T06:00:00+00:00");
        let conn = mem();
        let digest = PredictionDigest {
            habits: vec![(1, "用户 14 点左右开会".into(), 0.7)],
            ..Default::default()
        };
        let day1 = local(2026, 11, 19, 14);
        let id = match schedule_iteration(&conn, &digest, day1, false).unwrap() {
            ScheduleOutcome::Asked(id, _) => id,
            other => panic!("expected asked, got {other:?}"),
        };
        assert!(apply_feedback(&conn, id, "shut_up").unwrap());

        // Next 3 days: backoff.
        for offset in 1..=3 {
            let day = local(2026, 11, 19 + offset, 14);
            match schedule_iteration(&conn, &digest, day, false).unwrap() {
                ScheduleOutcome::Skipped(Some("backoff")) => {}
                other => panic!("day +{offset}: expected backoff, got {other:?}"),
            }
        }
        // Day 4: deliverable again.
        let day4 = local(2026, 11, 23, 14);
        assert!(matches!(
            schedule_iteration(&conn, &digest, day4, false).unwrap(),
            ScheduleOutcome::Asked(_, _)
        ));
    }

    #[test]
    fn feedback_after_48h_is_rejected() {
        let _clock = clock::pinned("2026-11-19T06:00:00+00:00");
        let conn = mem();
        let digest = PredictionDigest {
            habits: vec![(1, "用户 14 点左右开会".into(), 0.7)],
            ..Default::default()
        };
        let id = match schedule_iteration(&conn, &digest, local(2026, 11, 19, 14), false).unwrap() {
            ScheduleOutcome::Asked(id, _) => id,
            other => panic!("expected asked, got {other:?}"),
        };
        let _clock2 = clock::pinned("2026-11-23T06:00:00+00:00"); // > 48h later
        assert!(!apply_feedback(&conn, id, "hit").unwrap());
    }

    #[test]
    fn hit_feedback_bumps_preference_and_stats_track_accuracy() {
        let _clock = clock::pinned("2026-11-19T06:00:00+00:00");
        let conn = mem();
        let pid = habit(&conn, "用户 14 点左右开会", 0.6);
        let digest = PredictionDigest {
            habits: vec![(pid, "用户 14 点左右开会".into(), 0.6)],
            ..Default::default()
        };
        let id = match schedule_iteration(&conn, &digest, local(2026, 11, 19, 14), false).unwrap() {
            ScheduleOutcome::Asked(id, _) => id,
            other => panic!("expected asked, got {other:?}"),
        };
        assert!(apply_feedback(&conn, id, "hit").unwrap());
        let (_, _, conf) = db::active_habits(&conn).unwrap()[0];
        assert!((conf - 0.7).abs() < 1e-9, "hit bumps habit confidence +0.1");

        // Second day: miss.
        let _clock2 = clock::pinned("2026-11-20T06:00:00+00:00");
        let id2 = match schedule_iteration(&conn, &digest, local(2026, 11, 20, 14), false).unwrap() {
            ScheduleOutcome::Asked(id, _) => id,
            other => panic!("expected asked, got {other:?}"),
        };
        assert!(apply_feedback(&conn, id2, "miss").unwrap());

        let stats = db::prediction_stats(&conn).unwrap();
        assert_eq!(stats.hit, 1);
        assert_eq!(stats.miss, 1);
        assert_eq!(stats.asked, 2);
        assert!((stats.accuracy.unwrap() - 0.5).abs() < 1e-9);
        assert!((stats.accuracy_7d.unwrap() - 0.5).abs() < 1e-9);
        assert_eq!(stats.backoff_rate, Some(0.0));
    }

    #[test]
    fn record_skip_does_not_overwrite_an_asked_day() {
        let _clock = clock::pinned("2026-11-19T06:00:00+00:00");
        let conn = mem();
        let digest = PredictionDigest {
            habits: vec![(1, "用户 14 点左右开会".into(), 0.7)],
            ..Default::default()
        };
        schedule_iteration(&conn, &digest, local(2026, 11, 19, 14), false).unwrap();
        db::record_skip(&conn, "2026-11-19", "busy").unwrap();
        let row = db::prediction_on_day(&conn, "2026-11-19").unwrap().unwrap();
        assert!(row.asked_at.is_some(), "asked day must not be rewritten");
        assert!(row.skip_reason.is_none());
    }

    /// Release gates as a resident CI test (plan-predict.md §4): a compliant
    /// prediction history must read as 命中率 ≥ 50% and 退避率 < 10%; and the
    /// gate must actually reject a bad one (targets are enforced, not decor).
    const GATE_MIN_ACCURACY: f64 = 0.5;
    const GATE_MAX_BACKOFF: f64 = 0.1;

    #[test]
    fn metric_gates_accept_compliant_history_and_reject_bad() {
        let conn = mem();
        // 9 asked days: 8 hits, 1 miss, 0 shut_up → accuracy 0.89, backoff 0.
        for i in 0..9 {
            let _clock = clock::pinned(&format!("2026-11-{:02}T06:00:00+00:00", 10 + i));
            let row = db::record_prediction(
                &conn,
                &format!("2026-11-{:02}", 10 + i),
                "guess",
                "[]",
                14,
            )
            .unwrap();
            let verdict = if i == 4 { "miss" } else { "hit" };
            assert!(apply_feedback(&conn, row, verdict).unwrap());
        }
        let s = db::prediction_stats(&conn).unwrap();
        assert!(s.accuracy.unwrap() >= GATE_MIN_ACCURACY);
        assert!(s.backoff_rate.unwrap() < GATE_MAX_BACKOFF);

        // Bad history: half shut_up → gate rejects.
        let conn2 = mem();
        for i in 0..4 {
            let _clock = clock::pinned(&format!("2026-12-{:02}T06:00:00+00:00", 1 + i));
            let row = db::record_prediction(
                &conn2,
                &format!("2026-12-{:02}", 1 + i),
                "guess",
                "[]",
                14,
            )
            .unwrap();
            let verdict = if i < 2 { "shut_up" } else { "hit" };
            assert!(apply_feedback(&conn2, row, verdict).unwrap());
        }
        let s2 = db::prediction_stats(&conn2).unwrap();
        assert!(
            s2.backoff_rate.unwrap() >= GATE_MAX_BACKOFF,
            "50% shut_up must trip the back-off gate"
        );
    }
}

//! P1 item 2 — the form-line growth (docs/zh/plan-morph.md).
//!
//! The bonsai changes shape on a fixed timeline: four stages over the first
//! seven days of companionship, driven by TIME only (report §6.3 decision 2:
//! the form line is visible and pressure-free — no check-ins, no score).
//!
//! Invariants (plan §3.1):
//! - Stage only increases. Dormancy lowers the "complexion", never the stage.
//! - No death, only dormancy (report §6.3 decision 1): 7 days without any
//!   activity → wilted resting form; the first activity wakes it the same
//!   tick. Nothing is locked and nothing is lost.
//! - Growth is decided in Rust; the frontend only renders frames (same rule
//!   as the face state machine).
//! - Announcements never interrupt: they queue until the user OPENS the
//!   window (a natural breakpoint), never pop one up — the one-intrusion-
//!   per-day red line stays intact.

use chrono::Timelike;
use rusqlite::Connection;
use serde::Serialize;

use crate::memory::db;
use crate::memory::clock;

/// Stage thresholds by day index (0-based days since born_at):
/// S0 破土 days 0–1 · S1 抽枝 unlocks day 2 · S2 成形 day 4 · S3 成景 day 6.
pub const STAGE_THRESHOLDS: [i64; 4] = [0, 2, 4, 6];

pub const STAGE_NAMES: [&str; 4] = ["破土", "抽枝", "成形", "成景"];

/// Days without any activity before the resting (dormant) form sets in.
pub const DORMANT_AFTER_DAYS: i64 = 7;

/// Which event kinds count as "the user is around" for the dormancy rule.
/// Deliberately excludes machine-generated bookkeeping (`face_state` flips
/// with the clock even when nobody touches the machine; growth events are
/// our own writes).
pub const ACTIVITY_KINDS: &[&str] = &["dialogue", "perception", "prediction_asked", "prediction_feedback"];

// ---------------------------------------------------------------- derivation

/// Pure stage derivation from the day index — the 准时率 metric tests this.
pub fn derive_stage(day_index: i64) -> i64 {
    let mut stage = 0;
    for (i, threshold) in STAGE_THRESHOLDS.iter().enumerate() {
        if day_index >= *threshold {
            stage = i as i64;
        }
    }
    stage
}

/// Local calendar days elapsed since the birth anchor. 0-based: born today
/// = day 0. Unparsable anchors count as day 0 (grow from today, honestly).
pub fn day_index(born_at: &str, now_local: chrono::DateTime<chrono::Local>) -> i64 {
    chrono::DateTime::parse_from_rfc3339(born_at)
        .ok()
        .map(|t| (now_local.date_naive() - t.with_timezone(&chrono::Local).date_naive()).num_days())
        .unwrap_or(0)
        .max(0)
}

/// Announcement wording per stage, 3 tone templates cycled by the unlock day
/// (same trick as the predictor's `tone()`). 阿盆's pride sharpens as it grows.
fn announcement(stage: i64, day_index: i64) -> String {
    let variant = (day_index.max(0) as usize) % 3;
    match (stage, variant) {
        (1, 0) => "抽了根新枝。别乱碰，我自己都觉得好看。".into(),
        (1, 1) => "侧枝冒出来了。长个儿了，记一笔。".into(),
        (1, 2) => "新枝。你今天看了我几眼？我数着呢。".into(),
        (2, 0) => "成形了。树是慢慢长出来的，你那点耐心也是。".into(),
        (2, 1) => "树形齐了。看着我干什么，去干活。".into(),
        (2, 2) => "算是有模有样了。盆是我的，功劳是时间的。".into(),
        (3, 0) => "开花了。……别大惊小怪，正常发挥。".into(),
        (3, 1) => "成景了。可以晒，就晒一下下。".into(),
        _ => "开花了。七天，别人家盆景七年。行吧。".into(),
    }
}

/// Deterministic micro-variation seed for the frontend: same day → same
/// look, no RNG (growth must be honest, plan invariant 4).
pub fn micro_seed(day_index: i64) -> u64 {
    (day_index.max(0) as u64).wrapping_mul(6364136223846793005)
        ^ 0x9E3779B97F4A7C15
}

// ---------------------------------------------------------------- tick

/// One growth tick. Separated from the thread for testability; `now_local`
/// comes from the wall clock in production and a pinned clock in tests.
#[derive(Debug, Default, PartialEq)]
pub struct TickOutcome {
    /// (stage, note) when a new stage unlocked this tick.
    pub unlocked: Option<(i64, String)>,
    /// Some(true) = went dormant · Some(false) = woke up · None = no change.
    pub dormant_changed: Option<bool>,
    /// Always-recomputed view for the frontend event payload.
    pub day_index: i64,
    pub stage: i64,
    pub dormant: bool,
}

pub fn tick(
    conn: &Connection,
    now_local: chrono::DateTime<chrono::Local>,
) -> rusqlite::Result<TickOutcome> {
    let state = db::ensure_growth(conn)?;
    let today = now_local.format("%Y-%m-%d").to_string();
    let mut out = TickOutcome::default();

    // Stage advance: once per LOCAL day (last_tick_date makes it idempotent —
    // same trick as predictions' UNIQUE(date)).
    if state.last_tick_date.as_deref() != Some(today.as_str()) {
        let di = day_index(&state.born_at, now_local);
        let target = derive_stage(di);
        if target > state.stage {
            let note = announcement(target, di);
            db::set_growth_stage(conn, target)?;
            db::add_growth_log(conn, target, &note)?;
            db::add_event(
                conn,
                "growth_stage",
                &format!("{{\"stage\":{target},\"day\":{di}}}"),
            )?;
            out.unlocked = Some((target, note));
        }
        db::set_growth_tick_date(conn, &today)?;
        out.day_index = di;
        out.stage = target.max(state.stage);
    } else {
        out.stage = state.stage;
        out.day_index = day_index(&state.born_at, now_local);
    }

    // Dormancy: checked EVERY tick (a 60s poll, cheap). Rust-side date math
    // only — SQLite datetime comparisons with RFC3339 strings are a trap we
    // already fell into once (see plan-predict G4 notes).
    let cutoff = (now_local
        .with_timezone(&chrono::Utc)
        - chrono::Duration::days(DORMANT_AFTER_DAYS))
    .to_rfc3339();
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
    // A brand-new install has no history at all: that is "just sprouted",
    // not "abandoned". Dormancy only applies to a companion that HAS been
    // active before and then went quiet for the whole window.
    let historical: i64 = conn.query_row(
        &format!("SELECT COUNT(*) FROM events WHERE ts < ?1 AND kind IN ({kinds})"),
        rusqlite::params![cutoff],
        |r| r.get(0),
    )?;
    let dormant = active == 0 && historical > 0;
    let changed = if dormant {
        db::set_dormant(conn, Some(&clock::now_iso()))? > 0
    } else {
        db::set_dormant(conn, None)? > 0
    };
    if changed {
        out.dormant_changed = Some(dormant);
        db::add_event(
            conn,
            "growth_dormant",
            if dormant { "on" } else { "off" },
        )?;
    }

    out.dormant = if dormant {
        true
    } else {
        db::growth_state(conn)?.map(|s| s.dormant_since.is_some()).unwrap_or(false)
    };
    Ok(out)
}

// ---------------------------------------------------------------- spawn loop

/// Poll thread: every 60s, one growth tick. Unlocks and dormancy flips emit
/// `growth:stage` so an OPEN window updates immediately; a closed window
/// catches up via the pending announcement queue on the next open.
pub fn spawn(app: tauri::AppHandle) {
    use tauri::Manager;
    // T5 §5.6/A9 + plan-loop-supervisor：双层守卫；CAP 留痕；D1 有界复活。
    crate::failure::spawn_guarded("growth", move || {
        crate::failure::try_register(&app, "growth");
        let mut pg = crate::failure::PanicGuard::new("growth");
        loop {
            std::thread::sleep(std::time::Duration::from_secs(60));
            crate::failure::try_heartbeat(&app, "growth");
            let Some(store) = app.try_state::<crate::memory::MemoryStore>() else {
                continue;
            };
            let Ok(conn) = store.inner().0.lock() else {
                continue;
            };
            let now = chrono::Local::now();
            let _ = pg.tick(|| match tick(&conn, now) {
                Ok(out) => {
                    if out.unlocked.is_some() || out.dormant_changed.is_some() {
                        let payload = status_payload(&conn);
                        drop(conn);
                        use tauri::Emitter;
                        let _ = app.emit("growth:stage", payload);
                    }
                }
                Err(_) => {}
            });
            if pg.stopped() {
                if crate::failure::on_loop_cap(&app, "growth", "consecutive panic cap") {
                    spawn(app.clone());
                }
                break;
            }
        }
    });
}

/// The status view the frontend renders from (plan §3.5).
#[derive(Debug, Clone, Serialize)]
pub struct GrowthStatus {
    pub stage: i64,
    pub stage_name: String,
    pub day_index: i64,
    pub dormant: bool,
    pub born_at: String,
    /// Un-announced unlocks, oldest first.
    pub pending: Vec<PendingAnnouncement>,
    /// Deterministic seed for the day's micro-variations (leaf tint etc.).
    pub micro_seed: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct PendingAnnouncement {
    pub id: i64,
    pub stage: i64,
    pub note: String,
}

pub fn status_payload(conn: &Connection) -> GrowthStatus {
    let state = db::ensure_growth(conn).ok();
    let now = chrono::Local::now();
    let di = state
        .as_ref()
        .map(|s| day_index(&s.born_at, now))
        .unwrap_or(0);
    let pending = db::pending_growth_logs(conn)
        .unwrap_or_default()
        .into_iter()
        .map(|(id, stage, note)| PendingAnnouncement { id, stage, note })
        .collect();
    GrowthStatus {
        stage: state.as_ref().map(|s| s.stage).unwrap_or(0),
        stage_name: state
            .as_ref()
            .map(|s| s.stage)
            .map(|stage| STAGE_NAMES.get(stage as usize).unwrap_or(&"破土").to_string())
            .unwrap_or_else(|| "破土".to_string()),
        day_index: di,
        dormant: state.as_ref().map(|s| s.dormant_since.is_some()).unwrap_or(false),
        born_at: state.map(|s| s.born_at).unwrap_or_default(),
        pending,
        micro_seed: micro_seed(di),
    }
}

// ---------------------------------------------------------------- commands

/// Frontend boot + every window-open: the growth view. Pending announcements
/// are returned once — the caller marks them notified via `growth_ack`.
#[tauri::command]
pub fn growth_status(
    store: tauri::State<'_, crate::memory::MemoryStore>,
) -> Result<GrowthStatus, String> {
    let conn = store.inner().0.lock().map_err(|e| e.to_string())?;
    Ok(status_payload(&conn))
}

/// Acknowledge announcements the frontend actually rendered (idempotent:
/// only rows still pending flip, a double-open announces nothing twice).
#[tauri::command]
pub fn growth_ack(
    store: tauri::State<'_, crate::memory::MemoryStore>,
    id: i64,
) -> Result<bool, String> {
    let conn = store.inner().0.lock().map_err(|e| e.to_string())?;
    db::mark_growth_notified(&conn, id).map_err(|e| e.to_string())
}

/// S3 perk: a poke has a decent chance to flash the `proud` transient face
/// (deterministic on the wall-clock minute, plan invariant 4). S0–S2 pokes
/// keep the plain `greeted` behaviour.
pub fn poke_face(conn: &Connection, now: chrono::DateTime<chrono::Local>) -> crate::face::FaceState {
    let state = db::growth_state(conn).ok().flatten();
    let stage = state.map(|s| s.stage).unwrap_or(0);
    if stage >= 3 && (now.minute() % 2 == 0) {
        return crate::face::FaceState::Proud;
    }
    crate::face::FaceState::Greeted
}

// ---------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::clock;

    fn mem() -> Connection {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir = std::env::temp_dir().join("bonsai-morph-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("test-{}-{n}.sqlite3", std::process::id()));
        let _ = std::fs::remove_file(&path);
        db::open(&path).unwrap()
    }

    fn local(y: i32, m: u32, d: u32, h: u32) -> chrono::DateTime<chrono::Local> {
        use chrono::TimeZone;
        chrono::Local
            .with_ymd_and_hms(y, m, d, h, 0, 0)
            .single()
            .unwrap()
    }

    /// Release gate ① (plan §4): the stage must unlock on its exact day —
    /// day 2 → S1, day 4 → S2, day 6 → S3, nothing early, nothing late.
    #[test]
    fn stages_unlock_exactly_on_time() {
        assert_eq!(derive_stage(0), 0);
        assert_eq!(derive_stage(1), 0);
        assert_eq!(derive_stage(2), 1);
        assert_eq!(derive_stage(3), 1);
        assert_eq!(derive_stage(4), 2);
        assert_eq!(derive_stage(5), 2);
        assert_eq!(derive_stage(6), 3);
        assert_eq!(derive_stage(365), 3, "stage caps at S3");
    }

    #[test]
    fn day_index_counts_local_calendar_days() {
        let now = local(2026, 11, 19, 10);
        assert_eq!(day_index("2026-11-19T09:00:00+08:00", now), 0);
        assert_eq!(day_index("2026-11-13T09:00:00+08:00", now), 6);
        // Unparsable anchor → day 0 (grow from today).
        assert_eq!(day_index("garbage", now), 0);
    }

    #[test]
    fn first_tick_unlocks_and_records_log_once() {
        let _clock = clock::pinned("2026-11-13T01:00:00Z");
        let conn = mem();
        db::add_event(&conn, "perception", "app: X").unwrap(); // anchor day

        // Six days later: S3 must unlock with exactly ONE log row.
        let _clock2 = clock::pinned("2026-11-19T01:00:00Z");
        let out = tick(&conn, local(2026, 11, 19, 9)).unwrap();
        assert_eq!(out.unlocked.as_ref().unwrap().0, 3);
        assert_eq!(out.stage, 3);
        assert_eq!(db::pending_growth_logs(&conn).unwrap().len(), 1);
        let note = out.unlocked.unwrap().1;
        assert!(!note.is_empty(), "announcement must be non-empty");

        // A second tick the same day: no unlock, no new log (idempotent).
        let out2 = tick(&conn, local(2026, 11, 19, 15)).unwrap();
        assert_eq!(out2.unlocked, None);
        assert_eq!(db::pending_growth_logs(&conn).unwrap().len(), 1);
    }

    #[test]
    fn dormant_after_seven_silent_days_and_wakes_on_activity() {
        let _clock = clock::pinned("2026-11-01T01:00:00Z");
        let conn = mem();
        db::add_event(&conn, "dialogue", "hello").unwrap();

        // 7+ silent days → dormant (face_state bookkeeping must NOT count
        // as activity).
        let _clock2 = clock::pinned("2026-11-08T01:00:00Z");
        db::add_event(&conn, "face_state", "calm").unwrap();
        let out = tick(&conn, local(2026, 11, 9, 9)).unwrap();
        assert_eq!(out.dormant_changed, Some(true));
        assert!(out.dormant);

        // Still dormant while nothing happens.
        let out2 = tick(&conn, local(2026, 11, 10, 9)).unwrap();
        assert_eq!(out2.dormant_changed, None);
        assert!(out2.dormant);

        // First activity wakes it the same tick; the stage is untouched.
        let _clock3 = clock::pinned("2026-11-10T02:00:00Z");
        db::add_event(&conn, "dialogue", "我回来了").unwrap();
        let out3 = tick(&conn, local(2026, 11, 10, 10)).unwrap();
        assert_eq!(out3.dormant_changed, Some(false));
        assert!(!out3.dormant);
        assert_eq!(out3.stage, out.stage, "dormancy never rewinds the stage");
    }

    /// Release gate ② (plan §4): announcements are exactly-once. The
    /// must-fail case is the double window-open — this test IS that case.
    #[test]
    fn double_ack_announces_nothing_twice() {
        let _clock = clock::pinned("2026-11-13T01:00:00Z");
        let conn = mem();
        db::add_event(&conn, "dialogue", "hi").unwrap();
        let _clock2 = clock::pinned("2026-11-15T01:00:00Z");
        tick(&conn, local(2026, 11, 15, 9)).unwrap(); // S1 unlocks

        let payload = status_payload(&conn);
        assert_eq!(payload.pending.len(), 1);
        let id = payload.pending[0].id;
        assert!(db::mark_growth_notified(&conn, id).unwrap());
        let payload2 = status_payload(&conn);
        assert!(payload2.pending.is_empty(), "double open must not re-announce");
    }

    /// Release gate ③ (plan §4): dormancy correctness across the boundary —
    /// exactly DORMANT_AFTER_DAYS of silence must trigger, one activity
    /// event inside the window must prevent it.
    #[test]
    fn dormancy_boundary_is_exact() {
        let _clock = clock::pinned("2026-11-01T01:00:00Z");
        let conn = mem();
        db::add_event(&conn, "dialogue", "hi").unwrap();
        // 6 days 23h later: still inside the window.
        let _clock2 = clock::pinned("2026-11-08T00:00:00Z");
        let out = tick(&conn, local(2026, 11, 8, 9)).unwrap();
        assert!(!out.dormant);
        // 7 days + 1h: dormant.
        let out2 = tick(&conn, local(2026, 11, 8, 11)).unwrap();
        assert!(out2.dormant, "7 days of silence must trip dormancy");

        // And one perception inside the window keeps another plant awake.
        let conn2 = mem();
        let _clock3 = clock::pinned("2026-11-14T01:00:00Z");
        db::add_event(&conn2, "perception", "app: X").unwrap();
        let out3 = tick(&conn2, local(2026, 11, 20, 11)).unwrap();
        assert!(!out3.dormant);
    }

    #[test]
    fn s3_poke_is_deterministic_and_s0_poke_stays_greeted() {
        let _clock = clock::pinned("2026-11-19T01:00:00Z");
        let conn = mem();
        // S0: always greeted.
        assert_eq!(poke_face(&conn, local(2026, 11, 19, 10)), crate::face::FaceState::Greeted);
        assert_eq!(poke_face(&conn, local(2026, 11, 19, 11)), crate::face::FaceState::Greeted);

        // S3: minute parity decides, deterministically.
        db::ensure_growth(&conn).unwrap();
        db::set_growth_stage(&conn, 3).unwrap();
        let even = poke_face(&conn, local(2026, 11, 19, 10)); // minute 0
        let odd = poke_face(&conn, local(2026, 11, 19, 10).with_minute(1).unwrap());
        assert_eq!(even, crate::face::FaceState::Proud);
        assert_eq!(odd, crate::face::FaceState::Greeted);
        // Same minute → same answer (growth must be honest, no RNG).
        assert_eq!(poke_face(&conn, local(2026, 11, 19, 10)), even);
    }

    #[test]
    fn fresh_install_is_not_dormant() {
        // No events at all: just sprouted, not abandoned.
        let conn = mem();
        let out = tick(&conn, local(2026, 11, 19, 9)).unwrap();
        assert!(!out.dormant);
        assert_eq!(out.dormant_changed, None);
        assert_eq!(out.stage, 0);
    }

    #[test]
    fn micro_seed_is_stable_per_day_and_differs_across_days() {
        assert_eq!(micro_seed(4), micro_seed(4));
        assert_ne!(micro_seed(4), micro_seed(5));
        assert_eq!(micro_seed(-3), micro_seed(0), "negative day clamps to 0");
    }
}

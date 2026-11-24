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

use serde::Serialize;

/// Daily proactivity budget (L4 runtime state).
#[derive(Debug, Default)]
pub struct ProactivityState {
    pub used_today: u32,
    pub day: Option<chrono::NaiveDate>,
}

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

pub fn should_speak(
    input: &DecisionInput,
    state: &mut ProactivityState,
    daily_limit: u32,
) -> Decision {
    // Roll over the daily quota.
    let today = chrono::Utc::now().date_naive();
    if state.day != Some(today) {
        state.day = Some(today);
        state.used_today = 0;
    }

    if input.is_typing || input.is_fullscreen || input.in_meeting {
        return Decision::Silent(SilentReason::Busy);
    }
    if state.used_today >= daily_limit.max(0) {
        return Decision::Silent(SilentReason::QuotaExhausted);
    }
    if input.idle_minutes < MIN_IDLE_MINUTES {
        return Decision::Silent(SilentReason::UserActive);
    }
    if input.value - input.interruption_cost <= 0.0 {
        return Decision::Silent(SilentReason::NotWorthIt);
    }

    state.used_today += 1;
    Decision::Speak
}

// ---------------------------------------------------------------- command

/// Shared L4 evaluation: latest L1 signals + today's perception volume →
/// `should_speak`. Used by the command below and by the proactive loop
/// (dialogue.rs). The quota lives in the passed `ProactivityState`.
pub fn evaluate_live(
    conn: &rusqlite::Connection,
    signals: &crate::perception::LatestSignals,
    proactivity: &mut ProactivityState,
) -> Decision {
    let day = chrono::Local::now().date_naive();
    let signals_today = crate::report::local_day_window(day)
        .and_then(|(start, end)| {
            crate::memory::db::events_in_window(conn, &start, &end).ok()
        })
        .map(|events| events.iter().filter(|e| e.kind == "perception").count())
        .unwrap_or(0);
    let input = crate::perception::decision_input(
        signals,
        signals_today,
        crate::calendar::in_meeting_now(conn, chrono::Local::now()),
    );
    should_speak(&input, proactivity, DEFAULT_DAILY_LIMIT)
}

/// L4 entry: run `should_speak` on the latest L1 signals. The frontend (or a
/// future proactive loop) calls this; the daily quota lives in `AppState`, so
/// repeated calls respect the 1-per-day cap. L1 never decides on its own.
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
        .with_read(|conn| Ok(evaluate_live(conn, &signals, &mut proactivity)))
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> ProactivityState {
        ProactivityState::default()
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
        let mut s = state();
        let mut i = input(30, 0.9, 0.0);
        i.is_typing = true;
        assert_eq!(should_speak(&i, &mut s, 1), Decision::Silent(SilentReason::Busy));
    }

    #[test]
    fn quota_limits_to_one_per_day() {
        let mut s = state();
        let i = input(10, 0.9, 0.0);
        assert_eq!(should_speak(&i, &mut s, DEFAULT_DAILY_LIMIT), Decision::Speak);
        assert_eq!(
            should_speak(&i, &mut s, DEFAULT_DAILY_LIMIT),
            Decision::Silent(SilentReason::QuotaExhausted)
        );
    }

    #[test]
    fn requires_idle_over_five_minutes() {
        let mut s = state();
        assert_eq!(
            should_speak(&input(4, 0.9, 0.0), &mut s, 1),
            Decision::Silent(SilentReason::UserActive)
        );
        assert_eq!(should_speak(&input(5, 0.9, 0.0), &mut s, 1), Decision::Speak);
    }

    #[test]
    fn value_must_outweigh_cost() {
        let mut s = state();
        assert_eq!(
            should_speak(&input(30, 0.3, 0.5), &mut s, 1),
            Decision::Silent(SilentReason::NotWorthIt)
        );
        assert_eq!(should_speak(&input(30, 0.8, 0.2), &mut s, 1), Decision::Speak);
    }
}

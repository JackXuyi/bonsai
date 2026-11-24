//! L6 — Face state machine, owned by the Rust core (research report §8.2:
//! the decision layer decides, the presentation layer only renders frames).
//!
//! 12 frames = 6 emotional states × 2 frames each (calm / focused / sleepy /
//! greeted / displeased / thinking). The frontend renders poses; this module
//! decides which state is live and broadcasts changes as `face:state` events.
//!
//! State derivation follows persona quirks (阿盆: "深夜话变少"):
//! - 23:00–07:59 → Sleepy
//! - Weekday working hours (09–11, 14–17) → Focused
//! - otherwise → Calm
//!
//! Greeted / Displeased / Thinking / Proud are transient: they expire back
//! to the base state. L4 (growth events) drives `proud` via morph.rs (the
//! S3 stage perk); `face_preview` remains the manual hook.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

use chrono::{Datelike, Timelike};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FaceState {
    Calm,
    Focused,
    Sleepy,
    Greeted,
    Displeased,
    Thinking,
    /// S3 growth perk only (plan-morph.md §3.7): never in the base rotation.
    Proud,
}

impl FaceState {
    pub fn as_str(&self) -> &'static str {
        match self {
            FaceState::Calm => "calm",
            FaceState::Focused => "focused",
            FaceState::Sleepy => "sleepy",
            FaceState::Greeted => "greeted",
            FaceState::Displeased => "displeased",
            FaceState::Thinking => "thinking",
            FaceState::Proud => "proud",
        }
    }
}

/// Base state from local time. Pure function so it is unit-testable.
pub fn derive_base_state(hour: u8, is_weekend: bool) -> FaceState {
    match hour {
        23 | 0..=7 => FaceState::Sleepy,
        9..=11 | 14..=17 if !is_weekend => FaceState::Focused,
        _ => FaceState::Calm,
    }
}

const TRANSIENT_TTL: Duration = Duration::from_secs(4);

#[derive(Debug, Default)]
pub struct FaceController {
    transient: Option<(FaceState, Instant)>,
    last_emitted: Option<FaceState>,
}

impl FaceController {
    pub fn current(&self, base: FaceState) -> FaceState {
        match self.transient {
            Some((state, expiry)) if Instant::now() < expiry => state,
            _ => base,
        }
    }

    /// Show a transient state; it expires back to the base after the TTL.
    pub fn preview(&mut self, state: FaceState) {
        self.transient = Some((state, Instant::now() + TRANSIENT_TTL));
    }

    /// The user addressed the face directly.
    pub fn poke(&mut self) {
        self.preview(FaceState::Greeted);
    }

    /// Recompute against wall-clock time. Returns the state that should be
    /// broadcast (only when it differs from the last emitted one).
    pub fn tick(&mut self, hour: u8, is_weekend: bool) -> Option<FaceState> {
        let base = derive_base_state(hour, is_weekend);
        let current = self.current(base);
        if self.last_emitted != Some(current) {
            self.last_emitted = Some(current);
            Some(current)
        } else {
            None
        }
    }
}

pub fn is_weekend(now: &chrono::DateTime<chrono::Local>) -> bool {
    matches!(
        now.weekday(),
        chrono::Weekday::Sat | chrono::Weekday::Sun
    )
}

/// Background loop: every 5 s, recompute the base state, log changes to the
/// L2 memory store and broadcast `face:state` to the frontend.
pub fn spawn_tick_loop(handle: AppHandle) {
    // plan-loop-supervisor T1：双层守卫；CAP 留痕；D1 有界复活。
    crate::failure::spawn_guarded("face", move || {
        crate::failure::try_register(&handle, "face");
        let mut guard = crate::failure::PanicGuard::new("face");
        loop {
            std::thread::sleep(Duration::from_secs(5));
            crate::failure::try_heartbeat(&handle, "face");
            let _ = guard.tick(|| {
                let now = chrono::Local::now();
                let hour = now.hour() as u8;
                let weekend = is_weekend(&now);

                let controller = handle.state::<Mutex<FaceController>>();
                let mut controller = match controller.lock() {
                    Ok(c) => c,
                    Err(_) => return,
                };
                if let Some(state) = controller.tick(hour, weekend) {
                    drop(controller); // release before touching the memory store
                    let store = handle.state::<crate::memory::MemoryStore>();
                    if let Ok(conn) = store.inner().write_guard() {
                        let _ = crate::memory::db::add_event(&conn, "face_state", state.as_str());
                    }
                    let _ = handle.emit("face:state", state);
                }
            });
            if guard.stopped() {
                if crate::failure::on_loop_cap(&handle, "face", "consecutive panic cap") {
                    spawn_tick_loop(handle.clone());
                }
                break;
            }
        }
    });
}

#[tauri::command]
pub fn face_current(
    controller: tauri::State<'_, Mutex<FaceController>>,
) -> Result<FaceState, String> {
    let now = chrono::Local::now();
    let hour = now.hour() as u8;
    let base = derive_base_state(hour, is_weekend(&now));
    controller
        .inner()
        .lock()
        .map_err(|e| e.to_string())
        .map(|c| c.current(base))
}

/// The user clicked / addressed the face. From S3 onward (plan-morph.md
/// §3.7) a poke may flash the `proud` perk state instead of `greeted`.
#[tauri::command]
pub fn face_poke(handle: AppHandle) -> Result<FaceState, String> {
    let now = chrono::Local::now();
    let desired = {
        let store = handle.state::<crate::memory::MemoryStore>();
        match store.inner().write_guard() {
            Ok(conn) => crate::morph::poke_face(&conn, now),
            Err(_) => FaceState::Greeted,
        }
    };
    let controller = handle.state::<Mutex<FaceController>>();
    let state = {
        let mut c = controller.lock().map_err(|e| e.to_string())?;
        if desired == FaceState::Proud {
            c.preview(FaceState::Proud);
        } else {
            c.poke();
        }
        c.current(derive_base_state(now.hour() as u8, is_weekend(&now)))
    };
    let _ = handle.emit("face:state", state);
    Ok(state)
}

/// Force a transient state (manual preview today; later the hook for
/// L4 growth events and L5 disagreement reactions).
#[tauri::command]
pub fn face_preview(handle: AppHandle, state: FaceState) -> Result<FaceState, String> {
    let controller = handle.state::<Mutex<FaceController>>();
    let shown = {
        let mut c = controller.lock().map_err(|e| e.to_string())?;
        c.preview(state);
        c.current(derive_base_state(
            chrono::Local::now().hour() as u8,
            is_weekend(&chrono::Local::now()),
        ))
    };
    let _ = handle.emit("face:state", shown);
    Ok(shown)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn late_night_is_sleepy() {
        assert_eq!(derive_base_state(23, false), FaceState::Sleepy);
        assert_eq!(derive_base_state(3, false), FaceState::Sleepy);
        assert_eq!(derive_base_state(7, false), FaceState::Sleepy);
    }

    #[test]
    fn weekday_working_hours_are_focused() {
        assert_eq!(derive_base_state(9, false), FaceState::Focused);
        assert_eq!(derive_base_state(10, false), FaceState::Focused);
        assert_eq!(derive_base_state(14, false), FaceState::Focused);
        assert_eq!(derive_base_state(17, false), FaceState::Focused);
        // lunch break and outside work hours → calm
        assert_eq!(derive_base_state(12, false), FaceState::Calm);
        assert_eq!(derive_base_state(20, false), FaceState::Calm);
    }

    #[test]
    fn weekends_stay_calm_even_in_work_hours() {
        assert_eq!(derive_base_state(10, true), FaceState::Calm);
        assert_eq!(derive_base_state(15, true), FaceState::Calm);
    }

    #[test]
    fn transient_expires_back_to_base() {
        let mut c = FaceController::default();
        assert_eq!(c.current(FaceState::Calm), FaceState::Calm);

        c.poke();
        assert_eq!(c.current(FaceState::Calm), FaceState::Greeted);

        // simulate TTL expiry
        if let Some((_, expiry)) = c.transient.as_mut() {
            *expiry -= TRANSIENT_TTL + Duration::from_millis(1);
        }
        assert_eq!(c.current(FaceState::Calm), FaceState::Calm);
    }

    #[test]
    fn tick_emits_only_on_change() {
        let mut c = FaceController::default();
        assert_eq!(c.tick(10, false), Some(FaceState::Focused)); // initial emit
        assert_eq!(c.tick(10, false), None); // unchanged
        assert_eq!(c.tick(23, false), Some(FaceState::Sleepy)); // changed
    }

    #[test]
    fn tick_reports_transient_but_keeps_base_tracking() {
        let mut c = FaceController::default();
        c.tick(10, false);
        c.poke();
        // during the transient window the emitted state is Greeted…
        assert_eq!(c.current(FaceState::Focused), FaceState::Greeted);
        // …and after expiry it returns to the base
        if let Some((_, expiry)) = c.transient.as_mut() {
            *expiry -= TRANSIENT_TTL + Duration::from_millis(1);
        }
        assert_eq!(c.current(FaceState::Focused), FaceState::Focused);
    }
}

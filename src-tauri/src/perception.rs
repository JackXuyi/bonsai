//! L1 — Perception layer (report §8.2): 前台 App、idle、剪贴板、下载目录。
//!
//! Hard constraints from the research report:
//! - **逐项授权、默认关闭**: every signal source has its own switch and the
//!   whole layer is off until the user turns it on (`perception_set_config`).
//! - **原文不出机器**: signals land in the local SQLite events table as
//!   coarse labels only. Window titles are deliberately NOT captured —
//!   only the frontmost app name is.
//! - L1 feeds L4 (decision). Whether to speak ≠ what to say: this module
//!   only produces the *input* to `decision::should_speak`.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::decision::DecisionInput;
use crate::memory::MemoryStore;

/// How often the poll loop samples the environment.
pub const POLL_INTERVAL_SECS: u64 = 15;
/// Clipboard text above this is truncated before it is stored (bloat guard).
pub const MAX_CLIPBOARD_CHARS: usize = 1000;

// ---------------------------------------------------------------- config

/// Per-signal switches, persisted as JSON in the app data dir.
/// **Default all-off**: the user enables each source explicitly.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PerceptionConfig {
    pub enabled: bool,
    pub clipboard: bool,
    pub active_app: bool,
    pub downloads: bool,
}

impl Default for PerceptionConfig {
    fn default() -> Self {
        PerceptionConfig {
            enabled: false,
            clipboard: false,
            active_app: false,
            downloads: false,
        }
    }
}

impl PerceptionConfig {
    /// A source is active only when both the master switch and its own are on.
    pub fn source_on(&self, source: Source) -> bool {
        if !self.enabled {
            return false;
        }
        match source {
            Source::Clipboard => self.clipboard,
            Source::ActiveApp => self.active_app,
            Source::Downloads => self.downloads,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Source {
    Clipboard,
    ActiveApp,
    Downloads,
}

/// Load the persisted config, falling back to all-off on any error
/// (missing file, corrupt JSON) — fail safe toward "not watching".
pub fn load_config(path: &std::path::Path) -> PerceptionConfig {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_config(cfg: &PerceptionConfig, path: &std::path::Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    serde_json::to_string_pretty(cfg)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
        .and_then(|s| std::fs::write(path, s))
}

// ---------------------------------------------------------------- signals

/// The most recent environment snapshot, shared with the decision layer.
#[derive(Debug, Clone, Default, Serialize)]
pub struct LatestSignals {
    /// Seconds since the last keyboard/mouse input (macOS CGEventSource).
    pub idle_secs: f64,
    /// Frontmost application name (coarse; no window titles).
    pub front_app: Option<String>,
    pub updated_ts: String,
}

/// Managed state: latest signals, the poll collector's dedup memory, and
/// the persisted config path (so commands and the poll thread share it).
pub struct PerceptionState {
    pub latest: Mutex<LatestSignals>,
    pub collector: Mutex<Collector>,
    pub cfg_path: PathBuf,
}

// ---------------------------------------------------------------- collectors

#[cfg(target_os = "macos")]
mod os {
    // Direct CoreGraphics FFI — kCGEventSourceStateCombinedSessionState = 1,
    // kCGAnyInputEventType = -1. The `core-graphics` crate does not expose
    // this call, and a two-line FFI beats pulling in a wrapper for it.
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGEventSourceSecondsSinceLastEventType(state: u32, event_type: i32) -> f64;
    }

    /// Seconds since any user input (keyboard / mouse / scroll). `None` on
    /// API failure — callers treat it as "unknown".
    pub fn idle_secs() -> Option<f64> {
        let secs = unsafe { CGEventSourceSecondsSinceLastEventType(1, -1) };
        if secs.is_finite() && secs >= 0.0 {
            Some(secs)
        } else {
            None
        }
    }

    /// Frontmost application name via NSWorkspace. Coarse label only.
    pub fn front_app() -> Option<String> {
        active_win_pos_rs::get_active_window()
            .ok()
            .map(|w| w.app_name)
            .filter(|s| !s.trim().is_empty())
    }
}

#[cfg(not(target_os = "macos"))]
mod os {
    pub fn idle_secs() -> Option<f64> {
        None
    }
    pub fn front_app() -> Option<String> {
        None
    }
}

/// One sample of the real environment. Fields are `Option` so tests can
/// inject values; the runtime builder fills them from OS APIs.
#[derive(Debug, Default, Clone)]
pub struct OsInputs {
    pub idle: Option<f64>,
    pub app: Option<String>,
    pub clipboard: Option<String>,
}

impl OsInputs {
    /// Sample the actual OS (macOS collectors; clipboard via arboard).
    pub fn real(cfg: &PerceptionConfig) -> Self {
        OsInputs {
            idle: os::idle_secs(),
            app: if cfg.source_on(Source::ActiveApp) {
                os::front_app()
            } else {
                None
            },
            clipboard: if cfg.source_on(Source::Clipboard) {
                clipboard_text()
            } else {
                None
            },
        }
    }
}

/// Clipboard text. Any failure (empty, non-text, permissions) → None.
pub fn clipboard_text() -> Option<String> {
    arboard::Clipboard::new()
        .ok()?
        .get_text()
        .ok()
        .filter(|s| !s.trim().is_empty())
}

/// Truncate for storage, collapsing newlines so one-liner event rows stay sane.
pub fn clip_clipboard(text: &str, max: usize) -> String {
    let one_line: String = text
        .chars()
        .map(|c| if c == '\n' || c == '\r' || c == '\t' { ' ' } else { c })
        .collect();
    let truncated: String = one_line.chars().take(max).collect();
    if one_line.chars().count() > max {
        format!("{truncated}…")
    } else {
        truncated
    }
}

/// Cheap stable dedup key (FNV-1a) — no hash crate needed.
pub fn fnv1a(text: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in text.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// ~/Downloads, or None if HOME is unset.
fn downloads_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Downloads"))
}

/// Files currently in ~/Downloads (name + byte size), top level only.
fn list_downloads() -> Vec<(String, u64)> {
    let dir = match downloads_dir() {
        Some(d) => d,
        None => return Vec::new(),
    };
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            if let Ok(meta) = entry.metadata() {
                if meta.is_file() {
                    out.push((entry.file_name().to_string_lossy().into_owned(), meta.len()));
                }
            }
        }
    }
    out.sort();
    out
}

/// Dedup memory of the poll loop. `seen_downloads` is primed with the
/// directory contents at startup, so only files *arriving afterwards*
/// become events — boot never replays a whole day of old downloads.
#[derive(Default)]
pub struct Collector {
    last_clip_hash: Option<u64>,
    last_app: Option<String>,
    seen_downloads: HashSet<String>,
}

impl Collector {
    pub fn primed() -> Self {
        let mut c = Collector::default();
        for (name, _) in list_downloads() {
            c.seen_downloads.insert(name);
        }
        c
    }
}

/// One sampled event ready for the memory layer: `(content, source)`.
pub struct Sample {
    pub content: String,
    pub source: Source,
}

impl Collector {
    /// Sample the environment once, honoring per-source switches.
    /// OS values come from `inputs` (injectable for tests).
    pub fn tick(&mut self, cfg: &PerceptionConfig, inputs: &OsInputs) -> Vec<Sample> {
        let mut out = Vec::new();

        if cfg.source_on(Source::Clipboard) {
            if let Some(text) = &inputs.clipboard {
                let hash = fnv1a(text);
                if self.last_clip_hash != Some(hash) {
                    self.last_clip_hash = Some(hash);
                    out.push(Sample {
                        content: format!("clipboard: {}", clip_clipboard(text, MAX_CLIPBOARD_CHARS)),
                        source: Source::Clipboard,
                    });
                }
            }
        }

        if cfg.source_on(Source::ActiveApp) {
            if let Some(app) = &inputs.app {
                if self.last_app.as_deref() != Some(app.as_str()) {
                    self.last_app = Some(app.clone());
                    out.push(Sample {
                        content: format!("app: {app}"),
                        source: Source::ActiveApp,
                    });
                }
            }
        }

        if cfg.source_on(Source::Downloads) {
            for (name, size) in list_downloads() {
                if self.seen_downloads.insert(name.clone()) {
                    out.push(Sample {
                        content: format!("download: {name} ({size} bytes)"),
                        source: Source::Downloads,
                    });
                }
            }
        }

        out
    }
}

// ---------------------------------------------------------------- L4 wiring

/// Build the decision-layer input from the latest signals. Heuristics are
/// deliberately simple and documented (P0):
/// - typing ⇔ input in the last 3s;
/// - value rises with the number of signals collected today (something to
///   talk about) — 0 signals is still 0.3, a companion may just say hi;
/// - interruption cost is low: the companion lives in the menu bar.
pub fn decision_input(signals: &LatestSignals, signals_today: usize) -> DecisionInput {
    DecisionInput {
        idle_minutes: (signals.idle_secs / 60.0) as u32,
        is_typing: signals.idle_secs < 3.0,
        is_fullscreen: false, // P0: not cheaply detectable; stays permissive
        in_meeting: false,    // P1: calendar leg
        value: (0.3 + 0.1 * signals_today.min(6) as f32).min(0.9),
        interruption_cost: 0.2,
    }
}

// ---------------------------------------------------------------- runtime

/// Poll thread: sample → dedup → append perception events to the memory
/// layer → refresh LatestSignals. Runs forever; every source is skipped
/// unless both switches are on.
pub fn spawn(app: tauri::AppHandle, cfg_path: PathBuf) {
    std::thread::spawn(move || {
        use tauri::Manager;
        let mut collector = Collector::primed();
        loop {
            std::thread::sleep(std::time::Duration::from_secs(POLL_INTERVAL_SECS));
            let cfg = load_config(&cfg_path);
            let idle = os::idle_secs().unwrap_or(f64::MAX);
            let inputs = OsInputs::real(&cfg);

            let samples = if cfg.enabled {
                collector.tick(&cfg, &inputs)
            } else {
                Vec::new()
            };

            if let Some(store) = app.try_state::<MemoryStore>() {
                if let Ok(conn) = store.inner().0.lock() {
                    for s in &samples {
                        let _ = crate::memory::db::add_event(&conn, "perception", &s.content);
                    }
                }
            }

            if let Some(state) = app.try_state::<PerceptionState>() {
                let mut latest = state.latest.lock().unwrap_or_else(|e| e.into_inner());
                latest.idle_secs = idle;
                latest.front_app = inputs.app;
                latest.updated_ts = crate::memory::clock::now_iso();
            }
        }
    });
}

// ---------------------------------------------------------------- commands

#[tauri::command]
pub fn perception_get_config(state: tauri::State<'_, PerceptionState>) -> PerceptionConfig {
    load_config(&state.inner().cfg_path)
}

/// Set the config (one-key close = `enabled: false` + all sources off),
/// persist it, and return the effective config.
#[tauri::command]
pub fn perception_set_config(
    state: tauri::State<'_, PerceptionState>,
    cfg: PerceptionConfig,
) -> Result<PerceptionConfig, String> {
    save_config(&cfg, &state.inner().cfg_path).map_err(|e| e.to_string())?;
    Ok(cfg)
}

/// Live snapshot for the frontend (idle time, front app, layer on/off).
#[tauri::command]
pub fn perception_status(
    state: tauri::State<'_, PerceptionState>,
) -> Result<serde_json::Value, String> {
    let latest = state
        .inner()
        .latest
        .lock()
        .map_err(|e| e.to_string())?;
    Ok(serde_json::json!({
        "idle_secs": latest.idle_secs,
        "front_app": latest.front_app,
        "updated_ts": latest.updated_ts,
        "enabled": load_config(&state.inner().cfg_path).enabled,
    }))
}

// ---------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults_to_all_off() {
        let cfg = PerceptionConfig::default();
        assert!(!cfg.enabled);
        assert!(!cfg.source_on(Source::Clipboard));
        assert!(!cfg.source_on(Source::ActiveApp));
        assert!(!cfg.source_on(Source::Downloads));
    }

    #[test]
    fn source_needs_master_and_own_switch() {
        let mut cfg = PerceptionConfig::default();
        cfg.clipboard = true;
        assert!(!cfg.source_on(Source::Clipboard)); // master off
        cfg.enabled = true;
        assert!(cfg.source_on(Source::Clipboard));
        assert!(!cfg.source_on(Source::Downloads)); // own switch off
    }

    #[test]
    fn config_json_roundtrip_and_corrupt_fallback() {
        let dir = std::env::temp_dir().join("bonsai-perception-test");
        let path = dir.join("perception.json");
        let cfg = PerceptionConfig {
            enabled: true,
            clipboard: true,
            active_app: false,
            downloads: true,
        };
        save_config(&cfg, &path).unwrap();
        assert_eq!(load_config(&path), cfg);

        std::fs::write(&path, "{not json").unwrap();
        assert_eq!(load_config(&path), PerceptionConfig::default()); // fail safe off
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn clipboard_is_flattened_and_capped() {
        assert_eq!(clip_clipboard("a\nb\tc", 100), "a b c");
        let long = "x".repeat(1500);
        let clipped = clip_clipboard(&long, MAX_CLIPBOARD_CHARS);
        assert_eq!(clipped.chars().count(), MAX_CLIPBOARD_CHARS + 1); // + ellipsis
        assert!(clipped.ends_with('…'));
    }

    #[test]
    fn fnv1a_is_stable_and_discriminating() {
        assert_eq!(fnv1a("hello"), fnv1a("hello"));
        assert_ne!(fnv1a("hello"), fnv1a("hellp"));
    }

    #[test]
    fn tick_dedups_clipboard_and_app_but_reports_new_downloads() {
        let mut cfg = PerceptionConfig {
            enabled: true,
            clipboard: true,
            active_app: true,
            downloads: true,
        };
        let mut c = Collector::default();
        // First tick: one clipboard item, one app change. Downloads use the
        // real ~/Downloads listing (primed only in production spawn), which
        // in tests may add entries — asserted count-multiplicity, not names.
        let inputs = OsInputs {
            idle: Some(10.0),
            app: Some("Code".into()),
            clipboard: Some("hello".into()),
        };
        let out = c.tick(&cfg, &inputs);
        assert_eq!(out[0].content, "clipboard: hello");
        assert_eq!(out[1].content, "app: Code");

        // Same content again → only possible new download files, never
        // duplicate clipboard/app events.
        let again = c.tick(&cfg, &inputs);
        assert!(again.iter().all(|s| s.source == Source::Downloads));

        // App switch → exactly one app event (plus no clipboard dup).
        let inputs = OsInputs {
            idle: Some(10.0),
            app: Some("Safari".into()),
            clipboard: Some("hello".into()),
        };
        let out = c.tick(&cfg, &inputs);
        assert_eq!(
            out.iter().filter(|s| s.source != Source::Downloads).count(),
            1
        );
        assert!(out.iter().any(|s| s.content == "app: Safari"));

        // Master off → silent even with fresh content.
        cfg.enabled = false;
        let inputs = OsInputs {
            idle: Some(10.0),
            app: Some("Terminal".into()),
            clipboard: Some("fresh".into()),
        };
        assert!(c.tick(&cfg, &inputs).is_empty());
    }

    #[test]
    fn decision_input_maps_idle_and_value() {
        let mut s = LatestSignals::default();
        s.idle_secs = 2.0; // typing (input in the last 3s)
        let input = decision_input(&s, 0);
        assert!(input.is_typing);
        assert_eq!(input.idle_minutes, 0);
        assert!((input.value - 0.3).abs() < 1e-6);

        s.idle_secs = 400.0; // > 5 min idle
        let input = decision_input(&s, 5);
        assert!(!input.is_typing);
        assert_eq!(input.idle_minutes, 6);
        assert!((input.value - 0.8).abs() < 1e-6);
    }
}

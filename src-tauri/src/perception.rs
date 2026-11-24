//! L1 — Perception layer (report §8.2): 前台 App、idle、剪贴板、下载目录；
//! P2-4（plan-screen-l2/v1）起可选窗口标题（仅白名单 App + 教名词粗标签）。
//!
//! Hard constraints from the research report:
//! - **逐项授权、默认关闭**: every signal source has its own switch and the
//!   whole layer is off until the user turns it on (`perception_set_config`).
//! - **原文不出机器**: signals land in the local SQLite events table as
//!   coarse labels only. 默认只记前台 App 名；窗口标题须显式开启且 App
//!   在 `title_allowlist` 内，否则事件中不得出现窗口信息。
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
/// Window title coarse label cap (plan-screen-l2/v1).
pub const MAX_TITLE_CHARS: usize = 80;

// ---------------------------------------------------------------- config

/// User-taught noun: if the raw title contains `needle`, store `label` instead.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaughtNoun {
    pub needle: String,
    pub label: String,
}

/// Per-signal switches, persisted as JSON in the app data dir.
/// **Default all-off**: the user enables each source explicitly.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PerceptionConfig {
    pub enabled: bool,
    pub clipboard: bool,
    pub active_app: bool,
    pub downloads: bool,
    /// P2-4：是否采集窗口标题（仍须命中 allowlist）。
    #[serde(default)]
    pub window_title: bool,
    /// 允许写入 `window:` 事件的前台 App 名（与 OS 回报名大小写不敏感匹配）。
    #[serde(default)]
    pub title_allowlist: Vec<String>,
    /// 标题含子串 needle 时改存用户给的短 label。
    #[serde(default)]
    pub taught_nouns: Vec<TaughtNoun>,
}

impl Default for PerceptionConfig {
    fn default() -> Self {
        PerceptionConfig {
            enabled: false,
            clipboard: false,
            active_app: false,
            downloads: false,
            window_title: false,
            title_allowlist: Vec::new(),
            taught_nouns: Vec::new(),
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
            Source::WindowTitle => self.window_title,
        }
    }

    pub fn app_allowlisted(&self, app: &str) -> bool {
        let app = app.trim();
        if app.is_empty() {
            return false;
        }
        self.title_allowlist
            .iter()
            .any(|a| a.trim().eq_ignore_ascii_case(app))
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Source {
    Clipboard,
    ActiveApp,
    Downloads,
    /// P2-4 window title (allowlisted apps only).
    WindowTitle,
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
    /// Frontmost application name (coarse).
    pub front_app: Option<String>,
    /// Coarse window label when allowlisted + window_title on; else None.
    pub front_title: Option<String>,
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

    /// `(app_name, window_title)` — title may be empty without Screen Recording.
    pub fn front_window() -> Option<(String, String)> {
        active_win_pos_rs::get_active_window()
            .ok()
            .map(|w| (w.app_name, w.title))
            .filter(|(app, _)| !app.trim().is_empty())
    }
}

#[cfg(not(target_os = "macos"))]
mod os {
    pub fn idle_secs() -> Option<f64> {
        None
    }
    pub fn front_window() -> Option<(String, String)> {
        None
    }
}

/// One sample of the real environment. Fields are `Option` so tests can
/// inject values; the runtime builder fills them from OS APIs.
#[derive(Debug, Default, Clone)]
pub struct OsInputs {
    pub idle: Option<f64>,
    pub app: Option<String>,
    /// Raw window title from OS (may be empty); Collector decides allowlist.
    pub title: Option<String>,
    pub clipboard: Option<String>,
}

impl OsInputs {
    /// Sample the actual OS (macOS collectors; clipboard via arboard).
    pub fn real(cfg: &PerceptionConfig) -> Self {
        let need_front =
            cfg.source_on(Source::ActiveApp) || cfg.source_on(Source::WindowTitle);
        let (app, title) = if need_front {
            match os::front_window() {
                Some((a, t)) => (Some(a), Some(t)),
                None => (None, None),
            }
        } else {
            (None, None)
        };
        OsInputs {
            idle: os::idle_secs(),
            // window_title 也需要 app 名做白名单；active_app 关时仍可写 window:。
            app: if need_front { app } else { None },
            title: if cfg.source_on(Source::WindowTitle) {
                title
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

/// Resolve stored coarse label for a raw title (taught noun wins).
pub fn resolve_window_label(cfg: &PerceptionConfig, raw_title: &str) -> String {
    let t = raw_title.trim();
    if t.is_empty() {
        return "(无标题)".into();
    }
    let lower = t.to_lowercase();
    for n in &cfg.taught_nouns {
        let needle = n.needle.trim();
        let label = n.label.trim();
        if needle.is_empty() || label.is_empty() {
            continue;
        }
        if lower.contains(&needle.to_lowercase()) {
            return label.chars().take(40).collect();
        }
    }
    clip_clipboard(t, MAX_TITLE_CHARS)
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
    /// Dedup key: `(app, coarse_label)`.
    last_window: Option<(String, String)>,
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

        // P2-4：仅白名单 App 写 window:；未勾选 = 无窗口信息（S2/S4）。
        if cfg.source_on(Source::WindowTitle) {
            if let Some(app) = &inputs.app {
                if cfg.app_allowlisted(app) {
                    let raw = inputs.title.as_deref().unwrap_or("");
                    let label = resolve_window_label(cfg, raw);
                    let key = (app.clone(), label.clone());
                    if self.last_window.as_ref() != Some(&key) {
                        self.last_window = Some(key);
                        out.push(Sample {
                            content: format!("window: {app} | {label}"),
                            source: Source::WindowTitle,
                        });
                    }
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
/// - `in_meeting` comes from the calendar cache (plan-calendar.md §3.7):
///   two or more humans on the invite, currently running. No permission →
///   `false`, because a wrong silence is worse than a wrong word.
pub fn decision_input(
    signals: &LatestSignals,
    signals_today: usize,
    in_meeting: bool,
) -> DecisionInput {
    DecisionInput {
        idle_minutes: (signals.idle_secs / 60.0) as u32,
        is_typing: signals.idle_secs < 3.0,
        is_fullscreen: false, // P0: not cheaply detectable; stays permissive
        in_meeting,           // P2: calendar leg (plan-calendar.md §3.7)
        value: (0.3 + 0.1 * signals_today.min(6) as f32).min(0.9),
        interruption_cost: 0.2,
    }
}

// ---------------------------------------------------------------- runtime

/// Poll thread: sample → dedup → append perception events to the memory
/// layer → refresh LatestSignals. Runs forever; every source is skipped
/// unless both switches are on.
pub fn spawn(app: tauri::AppHandle, cfg_path: PathBuf) {
    // plan-loop-supervisor T1：双层守卫；CAP 留痕；D1 有界复活。
    crate::failure::spawn_guarded("perception", move || {
        use tauri::Manager;
        crate::failure::try_register(&app, "perception");
        let mut guard = crate::failure::PanicGuard::new("perception");
        let mut collector = Collector::primed();
        loop {
            std::thread::sleep(std::time::Duration::from_secs(POLL_INTERVAL_SECS));
            crate::failure::try_heartbeat(&app, "perception");
            let _ = guard.tick(|| {
                let cfg = load_config(&cfg_path);
                let idle = os::idle_secs().unwrap_or(f64::MAX);
                let inputs = OsInputs::real(&cfg);

                let samples = if cfg.enabled {
                    collector.tick(&cfg, &inputs)
                } else {
                    Vec::new()
                };

                if let Some(store) = app.try_state::<MemoryStore>() {
                    if let Ok(conn) = store.inner().write_guard() {
                        for s in &samples {
                            let _ = crate::memory::db::add_event(&conn, "perception", &s.content);
                        }
                    }
                }

                if let Some(state) = app.try_state::<PerceptionState>() {
                    let mut latest = state.latest.lock().unwrap_or_else(|e| e.into_inner());
                    latest.idle_secs = idle;
                    latest.front_app = if cfg.source_on(Source::ActiveApp) || cfg.source_on(Source::WindowTitle)
                    {
                        inputs.app.clone()
                    } else {
                        None
                    };
                    latest.front_title = match (&inputs.app, &inputs.title) {
                        (Some(app), Some(_))
                            if cfg.source_on(Source::WindowTitle) && cfg.app_allowlisted(app) =>
                        {
                            Some(resolve_window_label(
                                &cfg,
                                inputs.title.as_deref().unwrap_or(""),
                            ))
                        }
                        _ => None,
                    };
                    latest.updated_ts = crate::memory::clock::now_iso();
                }
            });
            if guard.stopped() {
                if crate::failure::on_loop_cap(&app, "perception", "consecutive panic cap") {
                    spawn(app.clone(), cfg_path.clone());
                }
                break;
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
    let cfg = load_config(&state.inner().cfg_path);
    Ok(serde_json::json!({
        "idle_secs": latest.idle_secs,
        "front_app": latest.front_app,
        "front_title": latest.front_title,
        "updated_ts": latest.updated_ts,
        "enabled": cfg.enabled,
        "window_title": cfg.window_title,
        "title_allowlist": cfg.title_allowlist,
        "taught_nouns": cfg.taught_nouns,
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
        assert!(!cfg.source_on(Source::WindowTitle));
        assert!(cfg.title_allowlist.is_empty());
        assert!(cfg.taught_nouns.is_empty());
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
            window_title: true,
            title_allowlist: vec!["Cursor".into()],
            taught_nouns: vec![TaughtNoun {
                needle: "bonsai".into(),
                label: "盆栽".into(),
            }],
        };
        save_config(&cfg, &path).unwrap();
        assert_eq!(load_config(&path), cfg);

        std::fs::write(&path, "{not json").unwrap();
        assert_eq!(load_config(&path), PerceptionConfig::default()); // fail safe off
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn old_config_without_screen_l2_fields_defaults() {
        let dir = std::env::temp_dir().join("bonsai-perception-legacy");
        let path = dir.join("perception.json");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            &path,
            r#"{"enabled":true,"clipboard":false,"active_app":true,"downloads":false}"#,
        )
        .unwrap();
        let cfg = load_config(&path);
        assert!(cfg.enabled && cfg.active_app);
        assert!(!cfg.window_title);
        assert!(cfg.title_allowlist.is_empty());
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
            ..Default::default()
        };
        let mut c = Collector::default();
        // First tick: one clipboard item, one app change. Downloads use the
        // real ~/Downloads listing (primed only in production spawn), which
        // in tests may add entries — asserted count-multiplicity, not names.
        let inputs = OsInputs {
            idle: Some(10.0),
            app: Some("Code".into()),
            clipboard: Some("hello".into()),
            ..Default::default()
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
            ..Default::default()
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
            ..Default::default()
        };
        assert!(c.tick(&cfg, &inputs).is_empty());
    }

    #[test]
    fn window_title_only_for_allowlisted_apps() {
        let cfg = PerceptionConfig {
            enabled: true,
            active_app: true,
            window_title: true,
            title_allowlist: vec!["Cursor".into()],
            ..Default::default()
        };
        let mut c = Collector::default();
        // S2: Safari 不在名单 → 仅 app:，无 window:
        let out = c.tick(
            &cfg,
            &OsInputs {
                app: Some("Safari".into()),
                title: Some("Apple".into()),
                ..Default::default()
            },
        );
        assert!(out.iter().any(|s| s.content == "app: Safari"));
        assert!(!out.iter().any(|s| s.source == Source::WindowTitle));

        // S1: Cursor 在名单 → window:
        let out = c.tick(
            &cfg,
            &OsInputs {
                app: Some("Cursor".into()),
                title: Some("bonsai — perception.rs".into()),
                ..Default::default()
            },
        );
        assert!(out.iter().any(|s| s.content == "app: Cursor"));
        assert!(out.iter().any(|s| {
            s.content
                .starts_with("window: Cursor | bonsai — perception.rs")
        }));

        // 同标题再 tick → 不去重窗口以外的重复
        let again = c.tick(
            &cfg,
            &OsInputs {
                app: Some("Cursor".into()),
                title: Some("bonsai — perception.rs".into()),
                ..Default::default()
            },
        );
        assert!(!again.iter().any(|s| s.source == Source::WindowTitle));
    }

    #[test]
    fn taught_noun_replaces_raw_title() {
        let cfg = PerceptionConfig {
            enabled: true,
            window_title: true,
            title_allowlist: vec!["Cursor".into()],
            taught_nouns: vec![TaughtNoun {
                needle: "bonsai".into(),
                label: "盆栽".into(),
            }],
            ..Default::default()
        };
        let mut c = Collector::default();
        let out = c.tick(
            &cfg,
            &OsInputs {
                app: Some("Cursor".into()),
                title: Some("edit docs/zh/plan-screen-l2 · bonsai".into()),
                ..Default::default()
            },
        );
        assert!(out.iter().any(|s| s.content == "window: Cursor | 盆栽"));
    }

    #[test]
    fn window_title_off_never_emits_window_events() {
        let cfg = PerceptionConfig {
            enabled: true,
            active_app: true,
            window_title: false,
            title_allowlist: vec!["Cursor".into()],
            ..Default::default()
        };
        let mut c = Collector::default();
        let out = c.tick(
            &cfg,
            &OsInputs {
                app: Some("Cursor".into()),
                title: Some("secret".into()),
                ..Default::default()
            },
        );
        assert!(out.iter().any(|s| s.content == "app: Cursor"));
        assert!(!out.iter().any(|s| s.content.contains("window:")));
    }

    #[test]
    fn decision_input_maps_idle_and_value() {
        let mut s = LatestSignals::default();
        s.idle_secs = 2.0; // typing (input in the last 3s)
        let input = decision_input(&s, 0, false);
        assert!(input.is_typing);
        assert_eq!(input.idle_minutes, 0);
        assert!((input.value - 0.3).abs() < 1e-6);

        s.idle_secs = 400.0; // > 5 min idle
        let input = decision_input(&s, 5, false);
        assert!(!input.is_typing);
        assert_eq!(input.idle_minutes, 6);
        assert!((input.value - 0.8).abs() < 1e-6);
    }
}

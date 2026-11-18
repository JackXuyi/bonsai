//! bonsai — a desktop AI companion that grows with you.
//!
//! Layered architecture (see docs/zh/research-report.md §8.2):
//! - L0 Shell:       Tauri 2, menu-bar resident, auto-start at login
//! - L1 Perception:  per-source switches, default off; local-only labels
//!   (front app / idle / clipboard / downloads) — see perception.rs
//! - L2 Memory:      local SQLite (events / facts / preferences / boundaries)
//! - L3 Personality: persona.json, model-independent
//! - L4 Decision:    "should it speak?" interruptibility model
//! - L5 Generation:  local-LLM dialogue + anti-sycophancy filter (scaffold)
//! - L6 Presentation: frontend in ../ui

pub mod decision;
pub mod face;
pub mod generation;
pub mod llm;
pub mod memory;
pub mod perception;
pub mod persona;
pub mod report;
pub mod tray;

use memory::MemoryStore;
use tauri::Manager;
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};

/// L4 runtime state shared across the app.
pub struct AppState {
    pub proactivity: std::sync::Mutex<decision::ProactivityState>,
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_autostart::init(
            // Research note (report §3.2): LaunchAgent and interactive launch must
            // share the same UID, or TCC permissions diverge. SMAppService is the
            // preferred long-term path; the plugin's LaunchAgent mode is the
            // scaffold default.
            MacosLauncher::LaunchAgent,
            Some(vec![]),
        ))
        .setup(|app| {
            // Boot to tray: start as a menu-bar resident, no Dock icon. The Dock
            // tile appears only while the face window is on screen
            // (see tray::show_main_window).
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            // L0: start at login so the companion is "there when the machine boots".
            // (Research note: boot with tray only, never pop a window.)
            let _ = app.autolaunch().enable();

            // L2: open / create the local SQLite store (data never leaves the machine).
            let db_path = app.path().app_data_dir()?.join("bonsai.sqlite3");
            if let Some(parent) = db_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let store = MemoryStore::open(&db_path)?;
            app.manage(store);

            // L1: perception — managed state (latest signals + collector with
            // primed downloads dedup) and the poll thread. Config is per-source
            // and defaults to all-off; the poll loop only records what the
            // user explicitly enabled.
            let cfg_path = db_path.parent().unwrap().join("perception.json");
            app.manage(perception::PerceptionState {
                latest: std::sync::Mutex::new(perception::LatestSignals::default()),
                collector: std::sync::Mutex::new(perception::Collector::primed()),
                cfg_path: cfg_path.clone(),
            });
            perception::spawn(app.handle().clone(), cfg_path);

            // L2: nightly consolidation — 03:00 local, catchup on restart.
            // (Report §4.3: "it thought about what you said last night" is a
            // real offline job, not a metaphor.)
            memory::scheduler::spawn(app.handle().clone());

            // L3: persona card is plain text + state machine — model-independent.
            // Swapping models swaps only expression, not the being.
            app.manage(persona::load());

            // L4: decision-layer runtime state.
            app.manage(AppState {
                proactivity: std::sync::Mutex::new(decision::ProactivityState::default()),
            });

            // L6: face state machine — Rust owns the state, the frontend only
            // renders the 12 frames (6 states × 2 frames).
            app.manage(std::sync::Mutex::new(face::FaceController::default()));
            face::spawn_tick_loop(app.handle().clone());

            // L0: tray icon + menu. The main window is created lazily on click,
            // keeping "boot to tray ready" fast.
            tray::setup(app)?;

            Ok(())
        })
        // Closing the window is "run in the background", not "quit": hide it and
        // fall back to tray-only so the companion keeps living in the menu bar.
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                tray::hide_main_window(window.app_handle());
            }
        })
        .invoke_handler(tauri::generate_handler![
            persona::get_persona,
            memory::add_event,
            memory::add_fact,
            memory::search,
            memory::consolidate,
            memory::forget_session,
            memory::forget_all,
            generation::scaffold_reply,
            face::face_current,
            face::face_poke,
            face::face_preview,
            perception::perception_get_config,
            perception::perception_set_config,
            perception::perception_status,
            decision::evaluate,
            report::daily_report,
        ])
        .run(tauri::generate_context!())
        .expect("error while running bonsai");
}

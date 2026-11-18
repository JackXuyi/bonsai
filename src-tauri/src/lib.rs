//! bonsai — a desktop AI companion that grows with you.
//!
//! Layered architecture (see docs/zh/research-report.md §8.2):
//! - L0 Shell:       Tauri 2, menu-bar resident, auto-start at login
//! - L1 Perception:  (not implemented in scaffold) raw text never leaves the machine
//! - L2 Memory:      local SQLite (events / facts / preferences / boundaries)
//! - L3 Personality: persona.json, model-independent
//! - L4 Decision:    "should it speak?" interruptibility model
//! - L5 Generation:  cloud dialogue + anti-sycophancy filter (scaffold)
//! - L6 Presentation: frontend in ../ui

pub mod decision;
pub mod generation;
pub mod memory;
pub mod persona;
pub mod tray;

use memory::MemoryStore;
use tauri::Manager;
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};

/// L4 runtime state shared across the app.
pub struct AppState {
    pub proactivity: decision::ProactivityState,
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
            // Menu-bar-resident app: no Dock icon on macOS.
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

            // L3: persona card is plain text + state machine — model-independent.
            // Swapping models swaps only expression, not the being.
            app.manage(persona::load());

            // L4: decision-layer runtime state.
            app.manage(AppState {
                proactivity: decision::ProactivityState::default(),
            });

            // L0: tray icon + menu. The main window is created lazily on click,
            // keeping "boot to tray ready" fast.
            tray::setup(app)?;

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            persona::get_persona,
            memory::add_event,
            memory::add_fact,
            memory::search,
            memory::consolidate,
            generation::scaffold_reply,
        ])
        .run(tauri::generate_context!())
        .expect("error while running bonsai");
}

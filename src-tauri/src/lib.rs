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

pub mod agent_tools;
pub mod calendar;
pub mod debug;
pub mod decision;
pub mod dialogue;
pub mod face;
pub mod filetools;
pub mod generation;
pub mod llm;
pub mod llm_admin;
pub mod llm_cloud;
pub mod llm_models;
pub mod llm_pull;
pub mod mcp;
pub mod mcp_admin;
pub mod mcp_discover;
pub mod memory;
pub mod memory_search;
pub mod morph;
pub mod onboarding;
pub mod perception;
pub mod persona;
pub mod predict;
pub mod relationship;
pub mod report;
pub mod skill;
pub mod summary;
pub mod tools;
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
            // Debug logs (spec-m4 §4B): init the logger FIRST so everything
            // after this line lands in app_log_dir/bonsai.log. (The spec
            // wanted pre-setup init, but the log dir needs the path resolver
            // that only exists inside setup; nothing before this logs.)
            if let Err(e) = debug::init(
                &app.path()
                    .app_log_dir()
                    .unwrap_or_else(|_| std::env::temp_dir()),
            ) {
                eprintln!("logger init failed: {e}");
            }

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

            // LLM config (spec-m1): UI settings win over env, survive restarts.
            // Backfill BEFORE the sidecar starts so `llm:ready` names the
            // configured model.
            {
                let store = app.state::<MemoryStore>();
                let conn = store.0.lock().map_err(|e| e.to_string())?;
                llm_admin::load_from_settings(&conn);
            }
            app.manage(llm_pull::PullState::default());

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

            // Agent tool pipeline (plan-agent-mcp-skill.md): live MCP
            // connections + the skills directory, managed once, shared by
            // admin commands and the dialogue turn.
            let skills_dir = db_path.parent().unwrap().join("skills");
            std::fs::create_dir_all(&skills_dir)?;

            // 内置总结 skill 包（plan-summary-skill）：只种一次。之后用户
            // 改过/删掉都尊重现状，不再复活。引擎不依赖文件存在——缺失时
            // 回落到编译进二进制的契约副本。
            {
                let store = app.state::<MemoryStore>();
                let conn = store.0.lock().map_err(|e| e.to_string())?;
                let seeded = memory::db::get_setting(&conn, "summary_skills_seeded")
                    .ok()
                    .flatten();
                if seeded.is_none() {
                    match skill::seed_builtin_packs(&skills_dir) {
                        Ok(created) => {
                            log::info!("summary skills seeded: {created:?}");
                            let _ = memory::db::set_setting(&conn, "summary_skills_seeded", "1");
                        }
                        Err(e) => log::warn!("summary skills seed failed: {e}"),
                    }
                }
            }

            app.manage(mcp_admin::AgentState {
                manager: mcp::McpManager::default(),
                skills_dir,
                approvals: std::sync::Arc::new(mcp_admin::ApprovalBroker::default()),
                app: Some(app.handle().clone()),
            });

            // MCP startup reconnect (2026-11-20 getHardwareInfo regression):
            // the connection pool only fills via the admin toggle, so a
            // restart left every enabled server "listed but not connected".
            // Background thread — connect latency (≤10s/server) must not
            // block boot; failures are logged, never fatal.
            {
                let handle = app.handle().clone();
                std::thread::spawn(move || {
                    let store = handle.state::<MemoryStore>();
                    let agent = handle.state::<mcp_admin::AgentState>();
                    let n = mcp_admin::reconnect_enabled_servers(&agent, &store);
                    if n > 0 {
                        log::info!("mcp_reconnect: {n} server(s) restored at startup");
                    }
                });
            }

            // L4: decision-layer runtime state.
            app.manage(AppState {
                proactivity: std::sync::Mutex::new(decision::ProactivityState::default()),
            });

            // LLM: spawn the bundled Ollama sidecar (best effort — missing
            // binary = degraded mode, rule legs keep working). Health check
            // runs in the background; `llm:ready` fires when the server is up.
            app.manage(llm::serve::ServerHandle::default());
            llm::serve::start(app.handle().clone());

            // L6: face state machine — Rust owns the state, the frontend only
            // renders the 12 frames (6 states × 2 frames).
            app.manage(std::sync::Mutex::new(face::FaceController::default()));
            face::spawn_tick_loop(app.handle().clone());

            // L5 dialogue pipeline + proactive loop: L4 owns the 1-per-day
            // quota and idle gates; this loop only speaks on a `Speak` verdict
            // (and stays silent when the local model is down).
            dialogue::spawn_proactive(app.handle().clone());

            // Predict-verify loop (P1 item 1, docs/zh/plan-predict.md):
            // one guess per day, evidence-backed, mutually exclusive with
            // the proactive chat channel.
            predict::spawn(app.handle().clone());

            // Form-line growth (P1 item 2, docs/zh/plan-morph.md): four
            // stages over seven days, time-driven, no death — only dormancy.
            morph::spawn(app.handle().clone());

            // Relationship line (P2 item 1, docs/zh/plan-relationship.md):
            // 拘谨 → 熟络 → 会顶嘴, driven by understanding accuracy only.
            relationship::spawn(app.handle().clone());

            // Calendar bridge (P2 item 2, docs/zh/plan-calendar.md): read-only,
            // default off, and the permission prompt is only ever triggered by
            // the user flipping the switch (a first-run prompt gets denied and
            // macOS denials are sticky). The source lives in this process on
            // purpose — TCC attributes permission to the responsible process.
            #[cfg(target_os = "macos")]
            let calendar_source: Box<dyn calendar::CalendarSource> =
                Box::new(calendar::EkCalendarSource { app: app.handle().clone() });
            #[cfg(not(target_os = "macos"))]
            let calendar_source: Box<dyn calendar::CalendarSource> =
                Box::new(calendar::MockCalendar::new(calendar::CalendarAccess::NotDetermined));
            app.manage(calendar::CalendarState { source: calendar_source });
            calendar::spawn(app.handle().clone());

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
            persona::snapshot::persona_snapshot,
            debug::debug_log_tail,
            debug::debug_open_log_dir,
            memory::memory_overview,
            generation::scaffold_reply,
            face::face_current,
            face::face_poke,
            face::face_preview,
            perception::perception_get_config,
            perception::perception_set_config,
            perception::perception_status,
            decision::evaluate,
            predict::prediction_feedback,
            predict::prediction_stats,
            morph::growth_status,
            morph::growth_ack,
            relationship::relationship_status,
            relationship::relationship_ack,
            relationship::relationship_reset,
            calendar::calendar_status,
            calendar::calendar_request_access,
            calendar::calendar_set_config,
            calendar::calendar_clear_cache,
            calendar::calendar_mute_today,
            calendar::calendar_open_settings,
            onboarding::onboarding_status,
            onboarding::onboarding_submit,
            onboarding::onboarding_reset,
            onboarding::boundary_list,
            onboarding::boundary_delete,
            dialogue::dialogue_chat,
            dialogue::dialogue_chat_stream,
            dialogue::dialogue_proactive_now,
            dialogue::dialogue_history,
            dialogue::turn_trace_get,
            dialogue::turn_trace_find_by_event,
            report::daily_report,
            summary::weekly_report,
            llm_admin::llm_config_get,
            llm_admin::llm_config_set_active,
            llm_models::llm_model_list_installed,
            llm_models::llm_catalog,
            llm_models::llm_model_remove,
            llm_pull::llm_model_download,
            llm_pull::llm_model_cancel,
            llm_cloud::llm_cloud_entries_get,
            llm_cloud::llm_cloud_entry_save,
            llm_cloud::llm_cloud_entry_remove,
            llm_cloud::llm_cloud_test,
            mcp_admin::mcp_server_list,
            mcp_admin::mcp_server_save,
            mcp_admin::mcp_server_remove,
            mcp_admin::mcp_server_toggle,
            mcp_admin::mcp_server_test,
            mcp_admin::mcp_server_tools,
            mcp_admin::builtin_tools_list,
            mcp_discover::mcp_discover_scan,
            mcp_discover::mcp_discover_import,
            mcp_admin::skill_list,
            mcp_admin::skill_toggle,
            mcp_admin::skill_remove,
            mcp_admin::tool_call_records,
            mcp_admin::mcp_approval_reply,
            mcp_admin::agent_approval_get,
            mcp_admin::agent_approval_set,
            filetools::fs_status,
            filetools::fs_set_config,
            filetools::fs_root_add,
            filetools::fs_root_remove,
        ])
        .build(tauri::generate_context!())
        .expect("error while running bonsai")
        .run(|app, event| {
            // Take the bundled LLM server down with us — nothing may leak.
            if let tauri::RunEvent::Exit = event {
                app.state::<llm::serve::ServerHandle>().stop();
                // An in-flight `ollama pull` would otherwise outlive the app.
                app.state::<llm_pull::PullState>().stop();
                // Live MCP child processes / HTTP sessions: bounded shutdown,
                // no orphans (spec-s1).
                app.state::<mcp_admin::AgentState>().manager.shutdown_all();
            }
        });
}

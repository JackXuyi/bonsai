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
pub mod context;
pub mod debug;
pub mod decision;
pub mod dialogue;
pub mod face;
pub mod faceskins;
pub mod failure;
pub mod filetools;
pub mod generation;
pub mod intent_gate;
pub mod intent_router;
pub mod action_rank;
pub mod task_recovery;
pub mod llm;
pub mod llm_admin;
pub mod llm_cloud;
pub mod llm_models;
pub mod llm_pull;
pub mod injection;
pub mod mail;
pub mod mcp;
pub mod observability;
pub mod mcp_admin;
pub mod mcp_discover;
pub mod memory;
pub mod memory_search;
pub mod morph;
pub mod onboarding;
pub mod perception;
pub mod perception_search;
pub mod persona;
pub mod predict;
pub mod ability;
pub mod relationship;
pub mod report;
pub mod session;
pub mod skill;
pub mod skill_auto;
pub mod skill_remote;
pub mod webtools;
pub mod summary;
pub mod tool_cache;
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
            let store = open_memory_store(&db_path)?;
            app.manage(store);

            // plan-loop-supervisor：进程内 loop 存活注册表（先于各 spawn 登记）。
            app.manage(failure::LoopRegistry::default());

            // LLM config (spec-m1): UI settings win over env, survive restarts.
            // Backfill BEFORE the sidecar starts so `llm:ready` names the
            // configured model.
            {
                let store = app.state::<MemoryStore>();
                let conn = store.write_guard().map_err(|e| e.to_string())?;
                // §5.6 启动清残留：崩溃留下的 status='running' 幽灵行标
                // interrupted（超 24h 才动，刚崩完的保守不动）。turn_trace
                // 无 assistant_event_id 的失败轮过滤留给 T7（统计口径在
                // 查询侧，本分支不动 memory/db.rs）。
                // Phase 3 A1：标 interrupted + interrupt_kind；D1 不自动续跑。
                match task_recovery::mark_stale_and_flag_resumable(&conn) {
                    Ok(0) => {}
                    Ok(n) => log::info!("startup cleanup: {n} stale 'running' task(s) marked interrupted"),
                    Err(e) => log::warn!("startup cleanup of stale running tasks failed: {e}"),
                }
                match task_recovery::list_resumable(&conn, 5) {
                    Ok(list) if !list.is_empty() => {
                        log::info!(
                            "startup: {} resumable task(s) available (explicit resume only)",
                            list.len()
                        );
                    }
                    _ => {}
                }
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
            // Mutex：显示名可在设置里改写（plan-presence-im v4）。
            let data_dir = app.path().app_data_dir()?;
            app.manage(std::sync::Mutex::new(persona::load_runtime(&data_dir)));

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
                let conn = store.write_guard().map_err(|e| e.to_string())?;
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
                session_allow: std::sync::Arc::new(mcp_admin::SessionToolAllow::default()),
                app: Some(app.handle().clone()),
            });

            // MCP startup reconnect (2026-11-20 getHardwareInfo regression):
            // the connection pool only fills via the admin toggle, so a
            // restart left every enabled server "listed but not connected".
            // Background thread — connect latency (≤10s/server) must not
            // block boot; failures are logged, never fatal.
            {
                let handle = app.handle().clone();
                // T5 §5.6：一次性后台线程也走守卫——panic 记日志，不炸进程。
                crate::failure::spawn_guarded("mcp_reconnect", move || {
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
            // §5.6 看门狗：模型请求失败 → probe 探活 → try_wait 确认退出
            // → 每小时至多 1 次自动重启，期间 emit `llm:restarting`。
            llm::watchdog::init(app.handle().clone());
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

            // P3-1 邮件（docs/zh/plan-mail/v1）：默认关；Automation 权限由用户
            // 开开关后首次读触发。MailSource 无状态，AppleScript 进程内执行。
            app.manage(mail::MailState {
                source: Box::new(mail::AppleMailSource),
            });

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
            persona::set_persona_name,
            memory::add_event,
            memory::add_fact,
            memory::search,
            memory::consolidate,
            memory::forget_session,
            memory::forget_all,
            memory::job::job_list,
            memory::job::job_set_enabled,
            session::session_list,
            session::session_create,
            session::session_rename,
            session::session_archive,
            session::session_delete,
            persona::snapshot::persona_snapshot,
            debug::debug_log_tail,
            debug::debug_open_log_dir,
            failure::loops_status,
            failure::loop_autorestart_get,
            failure::loop_autorestart_set,
            failure::failures_recent,
            memory::memory_overview,
            memory::store_lock_stats,
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
            ability::ability_status,
            ability::ability_ack,
            ability::ability_reset,
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
            memory::profile::profile_list,
            memory::profile::profile_set,
            memory::profile::profile_delete,
            dialogue::commands::dialogue_chat,
            dialogue::commands::dialogue_chat_stream,
            mcp_admin::runtime::dialogue_cancel,
            dialogue::commands::dialogue_proactive_now,
            dialogue::commands::dialogue_history,
            dialogue::commands::turn_trace_get,
            dialogue::commands::turn_trace_find_by_event,
            report::daily_report,
            summary::weekly_report,
            llm_admin::llm_config_get,
            llm_admin::llm_routing_set,
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
            mcp_admin::servers::mcp_server_list,
            mcp_admin::servers::mcp_server_save,
            mcp_admin::servers::mcp_server_remove,
            mcp_admin::servers::mcp_server_toggle,
            mcp_admin::servers::mcp_server_test,
            mcp_admin::servers::mcp_server_tools,
            mcp_admin::servers::builtin_tools_list,
            mcp_discover::mcp_discover_scan,
            mcp_discover::mcp_discover_import,
            mcp_admin::skill_cmds::skill_list,
            mcp_admin::skill_cmds::skill_toggle,
            mcp_admin::skill_cmds::skill_remove,
            mcp_admin::skill_cmds::skill_read,
            mcp_admin::skill_cmds::skill_import,
            mcp_admin::skill_cmds::skill_create,
            mcp_admin::skill_cmds::skill_update,
            mcp_admin::skill_cmds::skill_load_remote,
            mcp_admin::skill_cmds::skill_extract_now,
            mcp_admin::skill_cmds::skill_auto_candidates,
            mcp_admin::skill_cmds::skill_auto_dismiss,
            mcp_admin::skill_cmds::skill_auto_get_enabled,
            mcp_admin::skill_cmds::skill_auto_set_enabled,
            mcp_admin::gate_cmds::tool_call_records,
            mcp_admin::gate_cmds::plan_list,
            mcp_admin::gate_cmds::task_resume,
            mcp_admin::gate_cmds::task_reentry_choice,
            mcp_admin::gate_cmds::task_list_resumable,
            mcp_admin::gate_cmds::mcp_approval_reply,
            mcp_admin::gate_cmds::agent_approval_get,
            mcp_admin::gate_cmds::agent_approval_set,
            mcp_admin::gate_cmds::agent_budget_get,
            mcp_admin::gate_cmds::agent_budget_set,
            filetools::fs_status,
            filetools::fs_set_config,
            filetools::fs_root_add,
            filetools::fs_root_remove,
            filetools::fs_pick_directory,
            webtools::web_status,
            webtools::web_set_config,
            mail::mail_status,
            mail::mail_set_enabled,
            mail::mail_probe,
            observability::observability_day_export,
            faceskins::face_skin_list,
            faceskins::face_skin_create,
            faceskins::face_skin_pick_and_import,
            faceskins::face_skin_clear_slot,
            faceskins::face_skin_delete,
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

// ------------------------------------------------- 进程层恢复（plan-retry §5.6）

/// `SQLITE_BUSY` 的重试间隔（§5.6 / §11 风险矩阵：文件锁先重试再报错，
/// 绝不搬库）。
const BUSY_RETRY_DELAYS: [std::time::Duration; 3] = [
    std::time::Duration::from_secs(1),
    std::time::Duration::from_secs(2),
    std::time::Duration::from_secs(4),
];

/// L2 DB 打开的第二出口（plan-retry §5.6 / D5=A / A10）。
///
/// - `CORRUPT` / `NOTADB`：旧库文件改名备份（文件名带时间戳、保留原名可读）
///   → 建全新空库继续启动 → 写 `settings.db_recovered_at`（RFC3339）与
///   `settings.db_backup_path`（备份文件名）。**绝不静默**：这两个键是给
///   前端横幅的数据源（前端 T7 另做），用户必须知道「记忆是空的，旧库在这儿」。
/// - `SQLITE_BUSY`（文件锁）：按 1s/2s/4s 重试 3 次再报错，不进自愈。
/// - 其余错误原样上抛（`setup` Err → 应用起不来，维持现状语义）。
///
/// 只在调用侧改：memory/db.rs 的 schema/迁移代码一字不动。
fn open_memory_store(path: &std::path::Path) -> rusqlite::Result<MemoryStore> {
    let mut busy_retry = 0usize;
    loop {
        match MemoryStore::open(path) {
            Ok(store) => return Ok(store),
            Err(e) if failure::is_busy_db(&e) && busy_retry < BUSY_RETRY_DELAYS.len() => {
                let delay = BUSY_RETRY_DELAYS[busy_retry];
                busy_retry += 1;
                log::warn!(
                    "db busy（被别的进程锁着？）— retry {busy_retry}/{} in {delay:?}: {e}",
                    BUSY_RETRY_DELAYS.len()
                );
                std::thread::sleep(delay);
            }
            Err(e) if failure::is_corrupt_db(&e) => {
                log::error!("db corrupt（{e}）— 走第二出口：改名备份 + 建新库（plan-retry D5=A）");
                let backup = backup_corrupt_db(path).map_err(|io| {
                    rusqlite::Error::SqliteFailure(
                        rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CANTOPEN),
                        Some(format!("备份损坏库失败（拒绝静默重建）: {io}")),
                    )
                })?;
                let store = MemoryStore::open(path)?;
                {
                    let conn = store.write_guard().map_err(|_| {
                        rusqlite::Error::SqliteFailure(
                            rusqlite::ffi::Error::new(1),
                            Some("internal_poisoned: memory store mutex poisoned".into()),
                        )
                    })?;
                    // 写失败只 warn 不阻断：库已经建好了，横幅少个键好过起不来。
                    // 但绝不静默——日志里必须有这条 error 级的事实。
                    let recovered_at = chrono::Local::now().to_rfc3339();
                    if let Err(e) = memory::db::set_setting(&conn, "db_recovered_at", &recovered_at) {
                        log::error!("db recovery: 写 db_recovered_at 失败: {e}");
                    }
                    let backup_name = backup
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    if let Err(e) = memory::db::set_setting(&conn, "db_backup_path", &backup_name) {
                        log::error!("db recovery: 写 db_backup_path 失败: {e}");
                    }
                    log::warn!(
                        "db recovery 完成：旧库备份在 {}，新库为空（A10：用户必须被告知）",
                        backup.display()
                    );
                }
                return Ok(store);
            }
            Err(e) => return Err(e),
        }
    }
}

/// 把损坏的库（连同可能存在的 `-wal` / `-shm` 兄弟文件）改名备份。
/// 文件名形如 `bonsai.sqlite3.corrupt-20261124-153000.bak`：保留原名可读，
/// 时间戳防同名覆盖（同秒内第二次损坏会重定向失败——那种情况下本来
/// 也该停下来让人看一眼）。
fn backup_corrupt_db(path: &std::path::Path) -> std::io::Result<std::path::PathBuf> {
    let ts = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "bonsai.sqlite3".into());
    let dir = path.parent().unwrap_or(std::path::Path::new("."));
    let backup = dir.join(format!("{name}.corrupt-{ts}.bak"));
    std::fs::rename(path, &backup)?;
    for suffix in ["-wal", "-shm"] {
        let sibling = dir.join(format!("{name}{suffix}"));
        if sibling.exists() {
            let _ = std::fs::rename(&sibling, dir.join(format!("{name}{suffix}.corrupt-{ts}.bak")));
        }
    }
    Ok(backup)
}


#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("bonsai-t6-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    // ---------------- DB 第二出口（A10）

    #[test]
    fn corrupt_db_gets_backed_up_and_replaced_with_fresh() {
        // A10：手工造一个损坏库（垃圾字节 ≠ sqlite 头），应用必须起得来。
        let dir = tmp_dir("corrupt");
        let db = dir.join("bonsai.sqlite3");
        std::fs::write(&db, b"definitely not a sqlite database, just noise bytes").unwrap();

        let store = open_memory_store(&db).expect("损坏库必须走第二出口而不是报错");

        // 旧库被改名保留，文件名带时间戳、保留原名可读。
        let entries: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok().and_then(|e| e.file_name().into_string().ok()))
            .collect();
        let backup = entries
            .iter()
            .find(|n| n.starts_with("bonsai.sqlite3.corrupt-") && n.ends_with(".bak"))
            .expect("改名备份的文件必须在");
        assert!(!entries.contains(&"bonsai.sqlite3.corrupt.bak".to_string()));

        // 全新空库 + settings 两键（前端横幅的数据源，绝不静默）。
        let conn = store.write_guard().unwrap();
        let recovered_at = memory::db::get_setting(&conn, "db_recovered_at")
            .unwrap()
            .expect("db_recovered_at 必须有值");
        assert!(chrono::DateTime::parse_from_rfc3339(&recovered_at).is_ok(), "必须是 RFC3339");
        assert_eq!(
            memory::db::get_setting(&conn, "db_backup_path").unwrap().as_deref(),
            Some(backup.as_str()),
            "db_backup_path 存备份文件名"
        );
    }

    #[test]
    fn healthy_db_is_opened_without_any_recovery() {
        // 正常路径：不产生任何备份文件，也不写恢复键。
        let dir = tmp_dir("healthy");
        let db = dir.join("bonsai.sqlite3");

        let store = open_memory_store(&db).unwrap();
        let entries: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok().and_then(|e| e.file_name().into_string().ok()))
            .collect();
        assert!(!entries.iter().any(|n| n.contains(".corrupt-")));
        let conn = store.write_guard().unwrap();
        assert_eq!(memory::db::get_setting(&conn, "db_recovered_at").unwrap(), None);
    }

    // ---------------- 启动清残留 running

    #[test]
    fn stale_running_tasks_are_marked_interrupted() {
        let dir = tmp_dir("stale");
        let store = open_memory_store(&dir.join("bonsai.sqlite3")).unwrap();
        let conn = store.write_guard().unwrap();

        let old = (chrono::Utc::now() - chrono::Duration::hours(30)).to_rfc3339();
        let fresh = chrono::Utc::now().to_rfc3339();
        for (id, created_at, status) in [
            ("old-running", &old, "running"),
            ("fresh-running", &fresh, "running"),
            ("old-done", &old, "done"),
            ("fresh-failed", &fresh, "failed"),
        ] {
            conn.execute(
                "INSERT INTO tasks (id, created_at, goal, status) VALUES (?1, ?2, 'g', ?3)",
                [id, created_at, status],
            )
            .unwrap();
        }

        let n = task_recovery::mark_stale_and_flag_resumable(&conn).unwrap();
        assert_eq!(n, 1, "只有超 24h 的 running 被动");
        let status_of = |id: &str| -> String {
            conn.query_row("SELECT status FROM tasks WHERE id = ?1", [id], |r| r.get(0))
                .unwrap()
        };
        assert_eq!(status_of("old-running"), "interrupted");
        let kind: String = conn
            .query_row(
                "SELECT interrupt_kind FROM tasks WHERE id = 'old-running'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(kind, "crash");
        assert_eq!(status_of("fresh-running"), "running", "24h 内保守不动");
        assert_eq!(status_of("old-done"), "done", "非 running 不动");
        assert_eq!(status_of("fresh-failed"), "failed");
    }
}

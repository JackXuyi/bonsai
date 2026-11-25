//! Proactive talk loop (L4 decides; this module only speaks).

use std::time::Duration;

use chrono::Timelike;
use rusqlite::Connection;
use crate::decision::Decision;
use crate::face::{self, FaceState};
use crate::memory::db;
use crate::memory::MemoryStore;
use crate::perception::PerceptionState;
use crate::persona::{eval::DialogueBackend, Persona, PersonaState};
use crate::report;
use crate::AppState;

use super::persist::persist_model_turn;
use super::prompt::system_prompt;
use super::trace::TraceRecorder;
use super::types::{CHAT_TEMPERATURE, PROACTIVE_POLL_SECS, PROACTIVE_SESSION};

/// Today's observations as the proactive seed (counts, not raw dumps).
pub(crate) fn today_observations(conn: &Connection) -> String {
    let today = chrono::Local::now().date_naive();
    let d = report::collect(conn, today);
    let mut lines = Vec::new();
    if !d.clipboards.is_empty() {
        lines.push(format!("剪贴板出现 {} 条新内容", d.clipboards.len()));
    }
    if !d.downloads.is_empty() {
        lines.push(format!("下载了 {} 个文件", d.downloads.len()));
    }
    if let Some((app, _)) = d.apps.first() {
        lines.push(format!("最近在用 {app}"));
    }
    if !d.facts.is_empty() {
        lines.push(format!("盆栽今天新记住了 {} 件事", d.facts.len()));
    }
    if lines.is_empty() {
        "今天还没有记录到什么动静".to_string()
    } else {
        lines.join("；")
    }
}

/// Build the day's single proactive message. Returns `None` (silence) when
/// the model is unavailable — see module docs; `Some((text, turn_id))` when
/// it spoke (spec-s8: the trace row is created here, the event payload
/// carries the id so the frontend bubble can link to the trace page).
/// Records the message as an assistant dialogue event under the proactive
/// session.
///
/// Takes `&MemoryStore` (lock-narrowing): the model round below runs with
/// zero locks — it can take 10s+ on a cloud backend and must not block the
/// consolidate job or chat turns racing for the store.
pub fn proactive_text(
    store: &MemoryStore,
    backend: &mut dyn DialogueBackend,
    card: &Persona,
    hour: u32,
    raining: bool,
) -> rusqlite::Result<Option<(String, i64)>> {
    let (system, obs) = {
        let conn = store.write_guard()?;
        let s = system_prompt(&conn, card, hour, raining)?;
        let o = today_observations(&conn);
        (s, o)
    };
    let user = format!(
        "（系统注入的一次性指令：这是你今天唯一的主动搭话。基于下面的观察说一句自然、简短的话，像随口聊一句。不要列表，不要复述细节。实在没什么可说，就关心一句用户今天过得怎么样。）\n\n今日观察：{obs}"
    );
    // zero-lock model round
    let mut rec = TraceRecorder::with_user(&user);
    let t = std::time::Instant::now();
    let reply = backend.reply(&system, &[], &user, CHAT_TEMPERATURE);
    let dur = t.elapsed();
    let reply = reply.trim();
    if reply.is_empty() {
        return Ok(None); // model down → silence, not a canned line
    }
    rec.llm_round("主动搭话 · 今日观察", &system, reply, "answered", dur);
    let conn = store.write_guard()?;
    let asst_id =
        db::add_event_meta(&conn, "dialogue", reply, Some("assistant"), Some(PROACTIVE_SESSION))?;
    let turn_id = rec.persist(
        &conn,
        PROACTIVE_SESSION,
        None,
        Some(asst_id),
        "proactive",
        true,
        None,
        None,
        None,
        Some(reply),
    )?;
    Ok(Some((reply.to_string(), turn_id)))
}

// ---------------------------------------------------------------- proactive loop

/// One iteration of the proactive check: L1 signals + memory → L4 decision.
/// Separated from the thread for testability.
pub fn proactive_iteration(app: &tauri::AppHandle) -> Option<Decision> {
    use tauri::Manager;
    let store = app.try_state::<MemoryStore>()?;
    let perception = app.try_state::<PerceptionState>()?;
    let app_state = app.try_state::<AppState>()?;

    let signals = perception
        .inner()
        .latest
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let decision = {
        let conn = store.inner().write_guard().ok()?;
        let mut proactivity = app_state
            .proactivity
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        crate::decision::evaluate_live(&conn, &signals, &mut proactivity)
    };
    Some(decision)
}

/// Proactive thread: every minute, ask the decision layer once. L4 owns the
/// quota (1/day) and the idle/busy gates — this loop never opens its mouth
/// without a `Speak` verdict. On speak: build the message, emit
/// `proactive:message` to the frontend, record it in memory.
pub fn spawn_proactive(app: tauri::AppHandle) {
    use tauri::Manager;
    // plan-loop-supervisor T1：双层守卫；CAP 留痕；D1 有界复活。
    crate::failure::spawn_guarded("proactive", move || {
        crate::failure::try_register(&app, "proactive");
        let mut guard = crate::failure::PanicGuard::new("proactive");
        loop {
            std::thread::sleep(Duration::from_secs(PROACTIVE_POLL_SECS));
            crate::failure::try_heartbeat(&app, "proactive");
            let _ = guard.tick(|| {
                let Some(decision) = proactive_iteration(&app) else {
                    return;
                };
                if !matches!(decision, Decision::Speak) {
                    // D6：Rank 有候选也不降级强行开口——Decision Silent 本 tick 结束。
                    return;
                }

                let Some(store) = app.try_state::<MemoryStore>() else {
                    return; // evaluate 已持有同一 store；此处失败极端罕见
                };
                // Phase 3 A4：CandidateGen + Rank（与 Decision 分立）。
                // 日配额已由 should_speak 预留；此处无合格候选 → 释放配额沉默。
                // D6：本 tick 不因 Silent 降级到第二名（此处仅 Ambient 一种）。
                let day = chrono::Local::now().date_naive().format("%Y-%m-%d").to_string();
                let has_candidate = store.inner().write_guard().ok().is_some_and(|c| {
                    let obs = today_observations(&c);
                    let cands = crate::action_rank::generate_ambient(&obs, &day);
                    crate::action_rank::pick_top(cands, &[]).is_some()
                });
                if !has_candidate {
                    let _ = store.inner().with_write(|conn| {
                        crate::decision::release_reserve(conn)
                    });
                    return;
                }
                let Ok(mut backend) = crate::persona::backend() else {
                    let _ = store.inner().with_write(|conn| {
                        crate::decision::release_reserve(conn)
                    });
                    return;
                };
                let hour = chrono::Local::now().hour() as u32;
                let message = (|| -> Option<(String, i64)> {
                    let state = app.try_state::<PersonaState>()?;
                    let card = state.lock().ok()?.clone();
                    // proactive_text locks per phase itself — no guard held here.
                    proactive_text(store.inner(), backend.as_mut(), &card, hour, false)
                        .ok()
                        .flatten()
                })();

                if let Some((text, turn_id)) = message {
                    let _ = store.inner().with_write(|conn| {
                        crate::decision::commit_speak(conn)
                    });
                    use tauri::Emitter;
                    let _ = app.emit(
                        "proactive:message",
                        serde_json::json!({ "text": text, "turn_id": turn_id }),
                    );
                } else {
                    let _ = store.inner().with_write(|conn| {
                        crate::decision::release_reserve(conn)
                    });
                }
            });
            if guard.stopped() {
                if crate::failure::on_loop_cap(&app, "proactive", "consecutive panic cap") {
                    spawn_proactive(app.clone());
                }
                break;
            }
        }
    });
}



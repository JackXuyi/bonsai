//! L5 dialogue pipeline — the last assembly step of P0 item 5. Everything
//! here already existed; this module wires the chain:
//!
//! persona prompt (L3 renderer) → DialogueBackend (local LLM, or Mock in
//! tests) → anti-sycophancy filter (generation.rs) → memory layer events.
//!
//! Proactive talk follows the research constraints strictly (report §6.4):
//! the decision layer owns the 1-per-day quota and the idle gate; this
//! module only *says* something once L4 said "speak". If the local model is
//! down, proactive talk stays silent — silence is an explicit action, and a
//! canned line pretending to be alive is worse than none.
//!
//! User-initiated chat, in contrast, must always answer something: when the
//! model is unavailable it falls back to an honest "not myself right now"
//! line instead of pretending.

use std::time::Duration;

use chrono::Timelike;
use rusqlite::Connection;
use serde::Serialize;

use crate::decision::Decision;
use crate::face::{self, FaceState};
use crate::generation;
use crate::memory::db;
use crate::memory::MemoryStore;
use crate::perception::PerceptionState;
use crate::persona::prompt::{render, PromptContext};
use crate::persona::{eval::DialogueBackend, Persona};
use crate::report;
use crate::AppState;

/// Conversation window fed back to the model (plan §4.2).
const MAX_HISTORY_TURNS: usize = 6;
/// Dialogue temperature: personality wants some variance (eval harness
/// scores across 0.3/0.7/1.0; runtime sits on the middle grade).
const CHAT_TEMPERATURE: f64 = 0.7;
/// Session id for proactive messages (kept separate from user chats so a
/// "forget this conversation" can target either).
pub const PROACTIVE_SESSION: &str = "proactive";
/// How often the proactive loop checks the decision layer.
pub const PROACTIVE_POLL_SECS: u64 = 60;

#[derive(Debug, Serialize)]
pub struct DialogueOutcome {
    pub reply: String,
    pub filter: generation::FilterReport,
    /// False = model unavailable, the honest fallback line was used.
    pub model_used: bool,
}

const FALLBACK_REPLY: &str = "（盆栽这会儿状态不太好，脑子转不动——稍后再聊，别走开。）";

/// FaceState → the mood wording the persona prompt expects (report §5.1).
fn mood_str(state: FaceState) -> &'static str {
    match state {
        FaceState::Calm => "平静",
        FaceState::Focused => "专注",
        FaceState::Sleepy => "困",
        FaceState::Greeted => "被搭话",
        FaceState::Displeased => "不高兴",
        FaceState::Thinking => "思考",
    }
}

fn is_weekend_today() -> bool {
    use chrono::Datelike;
    matches!(
        chrono::Local::now().date_naive().weekday(),
        chrono::Weekday::Sat | chrono::Weekday::Sun
    )
}

fn system_prompt(
    conn: &Connection,
    card: &Persona,
    hour: u32,
    raining: bool,
) -> rusqlite::Result<String> {
    let boundaries = db::boundaries(conn)?;
    let hour8 = hour as u8;
    let mood = mood_str(face::derive_base_state(hour8, is_weekend_today()));
    Ok(render(
        card,
        &PromptContext {
            mood,
            hour,
            raining,
            user_boundaries: &boundaries,
        },
    ))
}

/// One user-initiated chat turn: render → backend → filter → record.
/// Always produces a reply (fallback line when the model is down).
pub fn chat_turn(
    conn: &Connection,
    backend: &mut dyn DialogueBackend,
    card: &Persona,
    session_id: &str,
    user_text: &str,
    hour: u32,
    raining: bool,
) -> rusqlite::Result<DialogueOutcome> {
    chat_turn_inner(conn, backend, card, session_id, user_text, hour, raining, None)
}

/// Same turn with incremental delivery: every model chunk goes to `on_delta`
/// while it is still being generated (the frontend renders it live).
pub fn chat_turn_streaming(
    conn: &Connection,
    backend: &mut dyn DialogueBackend,
    card: &Persona,
    session_id: &str,
    user_text: &str,
    hour: u32,
    raining: bool,
    on_delta: &mut dyn FnMut(&str),
) -> rusqlite::Result<DialogueOutcome> {
    chat_turn_inner(conn, backend, card, session_id, user_text, hour, raining, Some(on_delta))
}

fn chat_turn_inner(
    conn: &Connection,
    backend: &mut dyn DialogueBackend,
    card: &Persona,
    session_id: &str,
    user_text: &str,
    hour: u32,
    raining: bool,
    mut on_delta: Option<&mut dyn FnMut(&str)>,
) -> rusqlite::Result<DialogueOutcome> {
    let system = system_prompt(conn, card, hour, raining)?;
    let history = db::dialogue_history(conn, session_id, MAX_HISTORY_TURNS)?;
    let raw = match on_delta.as_deref_mut() {
        Some(cb) => backend.reply_streaming(&system, &history, user_text, CHAT_TEMPERATURE, cb),
        None => backend.reply(&system, &history, user_text, CHAT_TEMPERATURE),
    };
    let model_used = !raw.trim().is_empty();
    let reply = if model_used {
        raw
    } else {
        FALLBACK_REPLY.to_string()
    };
    let filter = generation::filter(&reply);
    db::add_event_meta(conn, "dialogue", user_text, Some("user"), Some(session_id))?;
    db::add_event_meta(conn, "dialogue", &reply, Some("assistant"), Some(session_id))?;
    Ok(DialogueOutcome { reply, filter, model_used })
}

/// Today's observations as the proactive seed (counts, not raw dumps).
fn today_observations(conn: &Connection) -> String {
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

/// Build the day's single proactive message. Returns None (silence) when
/// the model is unavailable — see module docs. Records the message as an
/// assistant dialogue event under the proactive session.
pub fn proactive_text(
    conn: &Connection,
    backend: &mut dyn DialogueBackend,
    card: &Persona,
    hour: u32,
    raining: bool,
) -> rusqlite::Result<Option<String>> {
    let system = system_prompt(conn, card, hour, raining)?;
    let obs = today_observations(conn);
    let user = format!(
        "（系统注入的一次性指令：这是你今天唯一的主动搭话。基于下面的观察说一句自然、简短的话，像随口聊一句。不要列表，不要复述细节。实在没什么可说，就关心一句用户今天过得怎么样。）\n\n今日观察：{obs}"
    );
    let reply = backend.reply(&system, &[], &user, CHAT_TEMPERATURE);
    let reply = reply.trim();
    if reply.is_empty() {
        return Ok(None); // model down → silence, not a canned line
    }
    db::add_event_meta(conn, "dialogue", reply, Some("assistant"), Some(PROACTIVE_SESSION))?;
    Ok(Some(reply.to_string()))
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
        let conn = store.inner().0.lock().ok()?;
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
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(PROACTIVE_POLL_SECS));
        let Some(decision) = proactive_iteration(&app) else {
            continue;
        };
        if !matches!(decision, Decision::Speak) {
            continue;
        }

        let Some(store) = app.try_state::<MemoryStore>() else {
            continue;
        };
        let Ok(mut backend) = crate::persona::backend() else {
            continue;
        };
        let hour = chrono::Local::now().hour() as u32;
        let message = (|| -> Option<String> {
            let card = app.try_state::<Persona>()?;
            let conn = store.inner().0.lock().ok()?;
            proactive_text(&conn, backend.as_mut(), card.inner(), hour, false)
                .ok()
                .flatten()
        })();

        if let Some(text) = message {
            use tauri::Emitter;
            let _ = app.emit("proactive:message", serde_json::json!({ "text": text }));
        }
    });
}

// ---------------------------------------------------------------- commands

#[tauri::command]
pub fn dialogue_chat(
    store: tauri::State<'_, MemoryStore>,
    card: tauri::State<'_, Persona>,
    user_text: String,
    session_id: Option<String>,
    raining: Option<bool>,
) -> Result<DialogueOutcome, String> {
    let session = session_id.unwrap_or_else(|| "default".to_string());
    let hour = chrono::Local::now().hour() as u32;
    let mut backend = crate::persona::backend().map_err(|e| e.to_string())?;
    let conn = store.inner().0.lock().map_err(|e| e.to_string())?;
    chat_turn(
        &conn,
        backend.as_mut(),
        &card,
        &session,
        &user_text,
        hour,
        raining.unwrap_or(false),
    )
    .map_err(|e| e.to_string())
}

/// Streaming chat command: deltas go to the frontend via the IPC channel
/// while the model is still generating; the final outcome (filter report,
/// fallback flag) returns once the reply is complete. The full reply is ALSO
/// in the outcome, so a client that ignores the channel still works.
#[tauri::command]
pub fn dialogue_chat_stream(
    store: tauri::State<'_, MemoryStore>,
    card: tauri::State<'_, Persona>,
    user_text: String,
    session_id: Option<String>,
    raining: Option<bool>,
    on_delta: tauri::ipc::Channel<serde_json::Value>,
) -> Result<DialogueOutcome, String> {
    let session = session_id.unwrap_or_else(|| "default".to_string());
    let hour = chrono::Local::now().hour() as u32;
    let mut backend = crate::persona::backend().map_err(|e| e.to_string())?;
    let conn = store.inner().0.lock().map_err(|e| e.to_string())?;
    let mut emit = |delta: &str| {
        let _ = on_delta.send(serde_json::json!({ "delta": delta }));
    };
    chat_turn_streaming(
        &conn,
        backend.as_mut(),
        &card,
        &session,
        &user_text,
        hour,
        raining.unwrap_or(false),
        &mut emit,
    )
    .map_err(|e| e.to_string())
}

/// Manual proactive trigger (frontend button / testing). Bypasses the daily
/// quota on purpose: an explicit user request is not proactivity.
#[tauri::command]
pub fn dialogue_proactive_now(
    store: tauri::State<'_, MemoryStore>,
    card: tauri::State<'_, Persona>,
) -> Result<Option<String>, String> {
    let hour = chrono::Local::now().hour() as u32;
    let mut backend = crate::persona::backend().map_err(|e| e.to_string())?;
    let conn = store.inner().0.lock().map_err(|e| e.to_string())?;
    proactive_text(&conn, backend.as_mut(), &card, hour, false).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persona::eval::MockBackend;
    use crate::persona::parse_card;

    fn card() -> Persona {
        parse_card(include_str!("../resources/persona.json")).expect("card parses")
    }

    fn mem() -> Connection {
        use std::sync::atomic::{AtomicU32, Ordering};
        static SEQ: AtomicU32 = AtomicU32::new(0);
        // Unique file per call: tests run in parallel inside one process, and
        // a shared path lets one test's remove_file yank rows from another.
        let dir = std::env::temp_dir().join("bonsai-dialogue-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!(
            "test-{}-{}.sqlite3",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::SeqCst)
        ));
        db::open(&path).unwrap()
    }

    #[test]
    fn chat_turn_records_both_sides_and_flags_sycophancy() {
        let conn = mem();
        let c = card();
        let mut backend = MockBackend::new("太棒了，就这么办！"); // sycophant draft
        let out =
            chat_turn(&conn, &mut backend, &c, "s1", "我想把所有记忆删了", 10, false).unwrap();
        assert!(out.model_used);
        assert!(out.filter.unconditional_affirmation, "filter must flag the opener");

        // Both sides recorded with the session id.
        let history = db::dialogue_history(&conn, "s1", 6).unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].user, "我想把所有记忆删了");
        assert_eq!(history[0].assistant, "太棒了，就这么办！");
        // Other sessions see nothing.
        assert!(db::dialogue_history(&conn, "s2", 6).unwrap().is_empty());
    }

    #[test]
    fn chat_turn_history_is_fed_back_across_turns() {
        let conn = mem();
        let c = card();
        let mut backend = MockBackend::new("好").script("第一轮", "回答甲").script("第二轮", "回答乙");
        chat_turn(&conn, &mut backend, &c, "s1", "第一轮", 10, false).unwrap();
        chat_turn(&conn, &mut backend, &c, "s1", "第二轮", 10, false).unwrap();
        let history = db::dialogue_history(&conn, "s1", 6).unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].assistant, "回答甲");
        assert_eq!(history[1].assistant, "回答乙");
    }

    #[test]
    fn dead_model_falls_back_to_honest_line() {
        let conn = mem();
        let c = card();
        // LocalBackend with no server → empty reply (see eval.rs docs).
        let mut backend = crate::persona::eval::LocalBackend::new();
        // Point at a closed port for determinism.
        backend.client = crate::llm::OllamaClient::new("http://127.0.0.1:1", "test");
        let out = chat_turn(&conn, &mut backend, &c, "s1", "在吗", 10, false).unwrap();
        assert!(!out.model_used);
        assert_eq!(out.reply, FALLBACK_REPLY);
    }

    #[test]
    fn proactive_speaks_with_mock_and_silences_on_dead_model() {
        let conn = mem();
        let c = card();

        let mut live = MockBackend::new("看你今天剪贴板挺忙，盆栽来问一句。");
        let text = proactive_text(&conn, &mut live, &c, 10, false).unwrap();
        assert!(text.is_some());
        // Recorded under the proactive session.
        let history = db::dialogue_history(&conn, PROACTIVE_SESSION, 6).unwrap();
        assert_eq!(history.len(), 1);

        let mut dead = crate::persona::eval::LocalBackend::new();
        dead.client = crate::llm::OllamaClient::new("http://127.0.0.1:1", "test");
        let text = proactive_text(&conn, &mut dead, &c, 10, false).unwrap();
        assert!(text.is_none(), "dead model → silence, not a canned line");
    }

    #[test]
    fn chat_turn_streaming_deltas_and_records_match_chat_turn() {
        let conn = mem();
        let c = card();
        let mut backend = MockBackend::new("分块也不会丢");
        let mut deltas = Vec::new();
        let out = chat_turn_streaming(
            &conn,
            &mut backend,
            &c,
            "s1",
            "在吗",
            10,
            false,
            &mut |d| deltas.push(d.to_string()),
        )
        .unwrap();
        // Default trait impl → one-shot: the whole reply arrives as one delta.
        assert_eq!(deltas, vec!["分块也不会丢".to_string()]);
        assert_eq!(out.reply, "分块也不会丢");
        assert!(out.model_used);
        let history = db::dialogue_history(&conn, "s1", 6).unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].assistant, "分块也不会丢");
    }

    #[test]
    fn mood_wording_covers_all_face_states() {
        assert_eq!(mood_str(FaceState::Calm), "平静");
        assert_eq!(mood_str(FaceState::Focused), "专注");
        assert_eq!(mood_str(FaceState::Sleepy), "困");
        assert_eq!(mood_str(FaceState::Greeted), "被搭话");
        assert_eq!(mood_str(FaceState::Displeased), "不高兴");
        assert_eq!(mood_str(FaceState::Thinking), "思考");
    }
}

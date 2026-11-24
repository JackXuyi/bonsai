//! L5 dialogue pipeline (plan-dialogue-split).
//!
//! persona prompt (L3) → DialogueBackend → anti-sycophancy filter → memory.
//! Submodules are a pure file split: behavior stays in `chat_turn_inner`.

mod types;
mod trace;
mod persist;
mod prompt;
mod cloud_loop;
mod local_legs;
mod summary_turn;
mod turn;
mod proactive;
/// Tauri commands live in a **public** submodule so `generate_handler!`
/// can see the inventory macros (`__cmd__*`) next to the fn items.
pub mod commands;

#[cfg(test)]
mod tests;

// ---- public API (stable paths for predict / frontend payload types) ----
pub use types::{
    DialogueOutcome, FailureInfo, ProactiveReply, TaskCard, TaskStep, PROACTIVE_POLL_SECS,
    PROACTIVE_SESSION,
};
pub use turn::{
    chat_turn, chat_turn_streaming, chat_turn_streaming_with_kit, chat_turn_with_kit,
};
pub use proactive::{proactive_iteration, proactive_text, spawn_proactive};

// ---- crate-visible re-exports so `tests` can `use super::*` ----
pub(crate) use types::{resolve_model_reply, CHAT_TEMPERATURE, FALLBACK_REPLY, INTERRUPTED_TAIL};
pub(crate) use trace::TraceRecorder;
pub(crate) use persist::{clip, filter_json, persist_model_turn};
pub(crate) use prompt::{
    env_prefix, is_weekend_today, mood_str, system_prompt, turn_budget, turns_to_history,
};
pub(crate) use cloud_loop::*;
pub(crate) use local_legs::*;
pub(crate) use summary_turn::*;
pub(crate) use turn::{call_backend_with_trace, chat_turn_inner};

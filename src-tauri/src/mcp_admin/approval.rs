//! Approval broker + ask-first gate (spec-s4).

use std::sync::{Arc, Condvar, Mutex};
use std::time::Instant;

use tauri::Emitter;

use crate::agent_tools::{
    format_args_summary, ApprovalDecision, ApprovalGate, ToolCall,
};

use super::state::SessionToolAllow;
use super::APPROVAL_WAIT;

/// One-at-a-time approval handshake (spec-s4): the chat turn reserves a slot
/// and blocks on it; the UI answers through `mcp_approval_reply`.
#[derive(Default)]
pub struct ApprovalBroker {
    inner: Mutex<BrokerInner>,
}

#[derive(Default)]
struct BrokerInner {
    seq: u64,
    pending: Option<Pending>,
}

struct Pending {
    id: u64,
    tool: String,
    slot: Arc<(Mutex<Option<bool>>, Condvar)>,
}

type Slot = Arc<(Mutex<Option<bool>>, Condvar)>;

impl ApprovalBroker {
    fn lock(&self) -> std::sync::MutexGuard<'_, BrokerInner> {
        // A poisoned broker must not turn into a silent allow — recover the
        // guard and keep the (still conservative) Deny behavior.
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Reserve the slot; returns the id shown on the card plus the wait slot.
    pub(crate) fn register(&self, tool: &str) -> (u64, Slot) {
        let mut inner = self.lock();
        inner.seq += 1;
        let id = inner.seq;
        let slot: Slot = Arc::new((Mutex::new(None), Condvar::new()));
        inner.pending = Some(Pending {
            id,
            tool: tool.to_string(),
            slot: slot.clone(),
        });
        (id, slot)
    }

    /// 测试用：当前是否仍有待应答卡。
    #[cfg(test)]
    pub(crate) fn has_pending(&self) -> bool {
        self.lock().pending.is_some()
    }

    /// Peek the pending tool name if `id` matches (or id is None).
    pub fn peek_tool(&self, id: Option<u64>) -> Option<String> {
        let inner = self.lock();
        let pending = inner.pending.as_ref()?;
        if let Some(want) = id {
            if want != pending.id {
                return None;
            }
        }
        Some(pending.tool.clone())
    }

    /// UI answer. `false` means nothing was waiting (stale card, double click,
    /// or the turn already timed out) — the caller keeps its Deny.
    pub fn reply(&self, id: Option<u64>, allow: bool) -> bool {
        let mut inner = self.lock();
        let Some(pending) = inner.pending.as_ref() else {
            return false;
        };
        if let Some(want) = id {
            if want != pending.id {
                return false;
            }
        }
        let slot = pending.slot.clone();
        inner.pending = None;
        let (lock, cv) = &*slot;
        *lock.lock().unwrap_or_else(|p| p.into_inner()) = Some(allow);
        cv.notify_all();
        true
    }

    fn clear(&self, id: u64) {
        let mut inner = self.lock();
        if inner.pending.as_ref().map(|p| p.id) == Some(id) {
            inner.pending = None;
        }
    }
}

/// Ask-first gate (spec-s4): emits `tool:approval_required` and waits up to
/// `APPROVAL_WAIT`（60s）for the inline bar. No app handle (headless / unit
/// tests) or a timeout ⇒ Deny — silence never becomes consent.
/// 本会话白名单命中时跳过弹卡（plan-chat-agent-inline v2）。
pub struct AskFirstGate {
    broker: Arc<ApprovalBroker>,
    app: Option<tauri::AppHandle>,
    session_allow: Arc<SessionToolAllow>,
}

impl AskFirstGate {
    pub fn new(
        broker: Arc<ApprovalBroker>,
        app: Option<tauri::AppHandle>,
        session_allow: Arc<SessionToolAllow>,
    ) -> Self {
        AskFirstGate {
            broker,
            app,
            session_allow,
        }
    }
}

impl ApprovalGate for AskFirstGate {
    fn check(&self, call: &ToolCall, source: &str, remote: bool) -> ApprovalDecision {
        if self.session_allow.allows(&call.name) {
            log::info!("approval_session_allow: tool={} → allow", call.name);
            return ApprovalDecision::Allow;
        }
        let Some(app) = self.app.as_ref() else {
            log::info!("approval_required(no-ui): tool={} → denied", call.name);
            return ApprovalDecision::Deny;
        };
        let (id, slot) = self.broker.register(&call.name);
        let args_summary = format_args_summary(&call.name, &call.args);
        let _ = app.emit(
            "tool:approval_required",
            serde_json::json!({
                "id": id,
                "tool": call.name,
                "source": source,
                "remote": remote,
                "argsSummary": args_summary,
            }),
        );

        let (lock, cv) = &*slot;
        let mut decided = lock.lock().unwrap_or_else(|p| p.into_inner());
        let deadline = Instant::now() + APPROVAL_WAIT;
        while decided.is_none() {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            let (guard, wait) = cv.wait_timeout(decided, left).unwrap_or_else(|p| p.into_inner());
            decided = guard;
            if wait.timed_out() {
                break;
            }
        }
        let allow = *decided == Some(true);
        drop(decided);
        self.broker.clear(id);
        log::info!("approval_answer: tool={} allow={}", call.name, allow);
        if allow {
            ApprovalDecision::Allow
        } else {
            ApprovalDecision::Deny
        }
    }
}

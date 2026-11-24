//! Shared dialogue types and constants (plan-dialogue-split).

use serde::Serialize;

use crate::failure::Failure;
use crate::generation;

/// Dialogue temperature: personality wants some variance (eval harness
/// scores across 0.3/0.7/1.0; runtime sits on the middle grade).
pub(crate) const CHAT_TEMPERATURE: f64 = 0.7;
/// Session id for proactive messages (kept separate from user chats so a
/// "forget this conversation" can target either). 值定义在 db 侧：会话索引
/// 的写入点是 `add_event_meta`，它必须知道哪个 id 不建行（plan-context D6），
/// 两处各写一份字符串迟早会走散。
pub use crate::memory::db::PROACTIVE_SESSION_ID as PROACTIVE_SESSION;
/// How often the proactive loop checks the decision layer.
pub const PROACTIVE_POLL_SECS: u64 = 60;
/// 任务卡（v12 §8 外壳）：一次云端工具循环的摘要，前端渲染进度卡。
#[derive(Debug, Clone, Serialize)]
pub struct TaskCard {
    pub id: String,
    pub status: String,
    pub steps: Vec<TaskStep>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TaskStep {
    pub seq: usize,
    pub tool: String,
    pub status: String,
}

/// 轮层失败载荷（plan-retry §7.1 / T7）：`kind` 决定文案与是否可重试，
/// 前端不再靠字符串 `includes` 猜 401/429/超时。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct FailureInfo {
    pub kind: String,
    pub message: String,
    pub retryable: bool,
}

impl From<&Failure> for FailureInfo {
    fn from(f: &Failure) -> Self {
        FailureInfo {
            kind: f.kind.as_str().to_string(),
            message: f.hint.to_string(),
            retryable: f.retryable,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct DialogueOutcome {
    pub reply: String,
    pub filter: generation::FilterReport,
    /// False = model unavailable, the honest fallback line was used.
    pub model_used: bool,
    /// True = the crisis intercept fired (plan-onboarding.md §3.3): the
    /// reply is the fixed resource card, no model was consulted.
    pub crisis: bool,
    /// Set when the turn was answered by a tool (plan-calendar.md §3.5):
    /// `"agenda_today" | "agenda_tomorrow" | "next" | "search"`. The frontend
    /// uses it to label the bubble instead of blaming a missing model.
    pub tool: Option<String>,
    /// 任务卡（v12）：本轮发生云端工具循环时非空——步骤清单 + 终态。
    pub task: Option<TaskCard>,
    /// Trace turn id (spec-s8): the frontend bubble links here for the
    /// generation-trace page. Always Some for turns recorded after the
    /// feature shipped.
    pub turn_id: Option<i64>,
    /// 本轮失败（plan-retry T7）。有值时 UI 按 `message`（hint）说话，
    /// 不再把 401/429/超时抹成同一句「脑子转不动」。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure: Option<FailureInfo>,
}

/// Honest fallback when the model is unavailable **and** no classified
/// [`Failure`] was captured (e.g. empty scripted reply with no error).
pub(crate) const FALLBACK_REPLY: &str = "（盆栽这会儿状态不太好，脑子转不动——稍后再聊，别走开。）";

/// 流式中途断的尾标（plan-retry §5.1 / A4）。
pub(crate) const INTERRUPTED_TAIL: &str = "——说到一半断了";

/// 把 `reply*` / 云循环的原文 + 可选 Failure 收成用户可见正文。
///
/// - 有正文 + Interrupted → 正文照留，尾附断流标记，`model_used=true`
/// - 有正文 + 其它失败 → 正文照留（部分成功），附 `failure`
/// - 空正文 + Failure → **用 hint**，不再抹成 [`FALLBACK_REPLY`]（A1–A3）
/// - 空正文且无 Failure → 才用兜底句
pub(crate) fn resolve_model_reply(
    raw: String,
    failure: Option<Failure>,
) -> (String, bool, Option<FailureInfo>) {
    let info = failure.as_ref().map(FailureInfo::from);
    let nonempty = !raw.trim().is_empty();
    match (nonempty, failure.as_ref()) {
        (true, Some(f)) if f.kind == crate::failure::FailureKind::Interrupted => {
            let reply = if raw.contains(INTERRUPTED_TAIL) {
                raw
            } else {
                format!("{raw}\n\n{INTERRUPTED_TAIL}")
            };
            (reply, true, info)
        }
        (true, _) => (raw, true, info),
        (false, Some(f)) => (f.hint.to_string(), false, info),
        (false, None) => (FALLBACK_REPLY.to_string(), false, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::failure::{Failure, FailureKind};

    #[test]
    fn empty_with_auth_uses_hint_not_fallback() {
        let f = Failure::new(FailureKind::Auth, "401");
        let (reply, used, info) = resolve_model_reply(String::new(), Some(f));
        assert!(!used);
        assert!(reply.contains("Key") || reply.contains("额度"), "{reply}");
        assert!(!reply.contains("脑子转不动"));
        assert_eq!(info.unwrap().kind, "auth");
    }

    #[test]
    fn interrupted_keeps_body_and_marks_tail() {
        let f = Failure::new(FailureKind::Interrupted, "cut");
        let (reply, used, info) = resolve_model_reply("说到一半的".into(), Some(f));
        assert!(used);
        assert!(reply.starts_with("说到一半的"));
        assert!(reply.contains(INTERRUPTED_TAIL));
        assert_eq!(info.unwrap().kind, "interrupted");
    }

    #[test]
    fn distinct_kinds_get_distinct_hints() {
        let a = resolve_model_reply(String::new(), Some(Failure::new(FailureKind::RateLimit, "429")));
        let b = resolve_model_reply(String::new(), Some(Failure::new(FailureKind::Timeout, "slow")));
        let c = resolve_model_reply(String::new(), Some(Failure::new(FailureKind::Server, "502")));
        assert_ne!(a.0, b.0);
        assert_ne!(b.0, c.0);
        assert_ne!(a.0, c.0);
    }
}

/// Manual proactive trigger result: the message plus its trace turn id.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProactiveReply {
    pub text: String,
    pub turn_id: i64,
}


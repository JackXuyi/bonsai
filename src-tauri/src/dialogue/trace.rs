//! In-memory turn trace collector (spec-s8).

use std::time::Duration;

use rusqlite::Connection;

use crate::memory::db;

/// In-memory collector for one turn's trace steps (spec-s8 §3.1). Steps
/// accumulate while the pipeline runs; the finished draft is persisted in
/// the SAME lock scope that writes the dialogue events (one transaction).
/// D1 (2026-11-20): prompts/outputs are stored full-text — no clipping.
pub(crate) struct TraceRecorder {
    started: std::time::Instant,
    steps: Vec<db::TraceStepDraft>,
    /// The current user message. It travels as the user message (NOT inside
    /// the system prompt), so without this field every trace step hides what
    /// the user actually asked (2026-11-21 /trace 盲点).
    user_text: Option<String>,
    /// T5 v17：本轮失败类别与重试次数（来自 error llm_round 步，落 turn_trace
    /// 两列——诊断页「最近一周失败分布」的数据源，plan-retry §6）。
    error_kind: Option<String>,
    attempts: Option<i64>,
}

impl TraceRecorder {
    pub(crate) fn new() -> Self {
        Self {
            started: std::time::Instant::now(),
            steps: Vec::new(),
            user_text: None,
            error_kind: None,
            attempts: None,
        }
    }

    /// Recorder that stamps the current user message onto every step.
    pub(crate) fn with_user(user: &str) -> Self {
        Self {
            started: std::time::Instant::now(),
            steps: Vec::new(),
            user_text: Some(user.into()),
            error_kind: None,
            attempts: None,
        }
    }

    /// One model round: title, full prompt, raw output, outcome label.
    pub(crate) fn llm_round(
        &mut self,
        title: &str,
        prompt: &str,
        output: &str,
        outcome: &str,
        dur: std::time::Duration,
    ) {
        if outcome == "error" {
            // v17：error 轮的 output 形如 "kind=… attempts=… detail…"——
            // 解析出来落 turn_trace 两列（解析失败只丢观测，不丢正文）。
            for part in output.split_whitespace() {
                if let Some(k) = part.strip_prefix("kind=") {
                    self.error_kind = Some(k.to_string());
                }
                if let Some(a) = part.strip_prefix("attempts=").and_then(|v| v.parse().ok()) {
                    self.attempts = Some(a);
                }
            }
        }
        self.steps.push(db::TraceStepDraft {
            step_kind: "llm_round".into(),
            title: title.into(),
            prompt: Some(prompt.into()),
            output: Some(output.into()),
            args_json: None,
            summary: None,
            outcome: Some(outcome.into()),
            duration_ms: Some(dur.as_millis() as i64),
            user_text: self.user_text.clone(),
        });
    }

    /// One audited tool execution.
    pub(crate) fn tool_call(
        &mut self,
        name: &str,
        args_json: &str,
        summary: &str,
        outcome: &str,
        output: Option<&str>,
        dur: std::time::Duration,
    ) {
        self.steps.push(db::TraceStepDraft {
            step_kind: "tool_call".into(),
            title: format!("工具调用 · {name}"),
            prompt: None,
            output: output.map(Into::into),
            args_json: Some(args_json.into()),
            summary: Some(summary.into()),
            outcome: Some(outcome.into()),
            duration_ms: Some(dur.as_millis() as i64),
            user_text: self.user_text.clone(),
        });
    }

    /// 规划轮（plan-planning T2）：一次独立的计划提交请求。step_kind 固定
    /// `plan_submit`，T5 的 trace 计划卡据此识别；失败也留 step（A8）。
    pub(crate) fn plan_round(&mut self, prompt: &str, output: &str, outcome: &str, dur: Duration) {
        self.steps.push(db::TraceStepDraft {
            step_kind: "plan_submit".into(),
            title: "规划轮".into(),
            prompt: Some(prompt.into()),
            output: Some(output.into()),
            args_json: None,
            summary: None,
            outcome: Some(outcome.into()),
            duration_ms: Some(dur.as_millis() as i64),
            user_text: self.user_text.clone(),
        });
    }

    /// T5 计划步骤状态：状态桥每次回写 plan_json 后记一条，output 是结构化
    /// JSON（seq/status/attempts/tool_call_id），前端 /trace 计划卡据此展示
    /// 步骤状态并关联真实 tool_calls 审计行（A10：无凭空步骤）。
    pub(crate) fn step_state(
        &mut self,
        seq: usize,
        status: &str,
        attempts: usize,
        tool_call_id: Option<i64>,
        outcome: &str,
    ) {
        let payload = serde_json::json!({
            "seq": seq,
            "status": status,
            "attempts": attempts,
            "tool_call_id": tool_call_id,
        });
        self.steps.push(db::TraceStepDraft {
            step_kind: "step_state".into(),
            title: format!("计划步骤 {} · {}", seq + 1, status),
            prompt: None,
            output: Some(payload.to_string()),
            args_json: None,
            summary: None,
            outcome: Some(outcome.into()),
            duration_ms: None,
            user_text: self.user_text.clone(),
        });
    }

    /// A non-model step (rule checks, retries, filter).
    pub(crate) fn simple(&mut self, step_kind: &str, title: &str, outcome: &str, output: Option<&str>) {
        self.steps.push(db::TraceStepDraft {
            step_kind: step_kind.into(),
            title: title.into(),
            prompt: None,
            output: output.map(Into::into),
            args_json: None,
            summary: None,
            outcome: Some(outcome.into()),
            duration_ms: None,
            user_text: self.user_text.clone(),
        });
    }

    /// Serialize the turn (master + steps) and return its id. `user` /
    /// `reply` feed the trace page's summary card (spec-s8 §5.3).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn persist(
        &self,
        conn: &Connection,
        session_id: &str,
        user_event_id: Option<i64>,
        assistant_event_id: Option<i64>,
        kind: &str,
        model_used: bool,
        tool_label: Option<String>,
        filter_report: Option<String>,
        user: Option<&str>,
        reply: Option<&str>,
    ) -> rusqlite::Result<i64> {
        db::trace_turn_persist(
            conn,
            &db::TraceTurnDraft {
                session_id: session_id.to_string(),
                kind: kind.into(),
                model_used,
                tool_label,
                filter_report,
                duration_ms: Some(self.started.elapsed().as_millis() as i64),
                user_text: user.map(Into::into),
                reply_text: reply.map(Into::into),
                error_kind: self.error_kind.clone(),
                attempts: self.attempts,
                steps: self.steps.clone(),
            },
            user_event_id,
            assistant_event_id,
        )
    }
}


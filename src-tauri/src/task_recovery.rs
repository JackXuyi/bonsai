//! Phase 3 Autonomy · A1/A2 — Task Recovery + Step Idempotency
//! （docs/zh/plan-autonomy-phase3/v1）。
//!
//! 纯裁决与 DB 准备路径：不直接执工具。续跑副作用仍须经 `execute_audited`。

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::agent_tools::ToolAccess;
use crate::memory::clock;
use crate::memory::db::{self, PlanJson, PlanStep};

/// 审计行是否算「工具侧成功」。
pub fn audit_outcome_ok(outcome: &str) -> bool {
    matches!(outcome, "ok")
}

/// 一步是否 checkpoint-complete（计划 done + 审计存在且 ok）。
pub fn is_checkpoint_complete(step: &PlanStep, audit_outcome: Option<&str>) -> bool {
    step.status == "done"
        && step.tool_call_id.is_some()
        && audit_outcome.map(audit_outcome_ok).unwrap_or(false)
}

/// 效应分级：未知按 write 保守（方案 §5.1）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectClass {
    Read,
    Write,
    Ask,
    Unknown,
}

impl EffectClass {
    pub fn from_str_opt(s: Option<&str>) -> Self {
        match s.map(|x| x.trim().to_ascii_lowercase()).as_deref() {
            Some("read") => Self::Read,
            Some("write") => Self::Write,
            Some("ask") => Self::Ask,
            _ => Self::Unknown,
        }
    }

    pub fn from_access(access: ToolAccess) -> Self {
        match access {
            ToolAccess::Read => Self::Read,
            ToolAccess::Ask => Self::Write, // Ask 档含写副作用风险 → 按 write
            ToolAccess::Forbidden => Self::Unknown,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Ask => "ask",
            Self::Unknown => "unknown",
        }
    }
}

/// 跨进程重入裁决（方案 §5.2）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReentryDecision {
    /// 已有成功证明 → 标 done，不调用工具。
    Skip,
    /// 允许有界重试（仅 read）。
    Retry,
    /// 无法证明 → 问用户。
    AskUser,
    /// 明确不可重放。
    Fail,
}

/// 纯函数：根据步骤状态、审计、效应、是否已有同幂等键成功审计，裁决重入。
pub fn reentry_decision(
    step_status: &str,
    audit_outcome: Option<&str>,
    effect: EffectClass,
    idempotent_success_exists: bool,
) -> ReentryDecision {
    if idempotent_success_exists || is_checkpoint_complete(
        &PlanStep {
            seq: 0,
            intent: String::new(),
            tool_hint: String::new(),
            status: step_status.to_string(),
            attempts: 0,
            tool_call_id: Some(1),
            idempotency_key: None,
            effect_class: None,
        },
        audit_outcome,
    ) {
        return ReentryDecision::Skip;
    }
    // 审计 ok 但计划未标 done → 补提交（调用方 heal），此处当 Skip。
    if audit_outcome.map(audit_outcome_ok).unwrap_or(false) {
        return ReentryDecision::Skip;
    }
    match effect {
        EffectClass::Read => ReentryDecision::Retry,
        EffectClass::Write | EffectClass::Ask | EffectClass::Unknown => {
            if step_status == "done" && audit_outcome.is_none() {
                // 计划 done 无审计 → 降级未完成，写类问用户
                ReentryDecision::AskUser
            } else if matches!(step_status, "running" | "pending" | "") {
                ReentryDecision::AskUser
            } else if step_status == "failed" {
                ReentryDecision::AskUser
            } else {
                ReentryDecision::Fail
            }
        }
    }
}

/// 分裂修复：审计 ok / 计划未提交 → 补 done；计划 done 无审计 → 改 pending。
pub fn heal_plan_checkpoints(
    plan: &mut PlanJson,
    audit_by_id: &dyn Fn(i64) -> Option<String>,
) -> bool {
    let mut changed = false;
    for step in &mut plan.steps {
        if let Some(id) = step.tool_call_id {
            let outcome = audit_by_id(id);
            if outcome.as_deref().map(audit_outcome_ok).unwrap_or(false) {
                if step.status != "done" {
                    step.status = "done".into();
                    changed = true;
                }
            }
        } else if step.status == "done" {
            // 计划 done、无 tool_call_id → 降级
            step.status = "pending".into();
            changed = true;
        } else if step.status == "running" {
            // 崩溃在执行中、无审计
            step.status = "pending".into();
            changed = true;
        }
    }
    changed
}

/// 最大满足 checkpoint-complete 的 seq；若无则 -1。
pub fn last_safe_seq(
    plan: &PlanJson,
    audit_by_id: &dyn Fn(i64) -> Option<String>,
) -> i32 {
    let mut best = -1;
    for step in &plan.steps {
        let outcome = step.tool_call_id.and_then(|id| audit_by_id(id));
        if is_checkpoint_complete(step, outcome.as_deref()) {
            best = best.max(step.seq);
        }
    }
    best
}

/// 下一待执行步（safe+1），若已全部完成则 None。
pub fn next_resume_step<'a>(plan: &'a PlanJson, last_safe: i32) -> Option<&'a PlanStep> {
    let mut candidates: Vec<&PlanStep> = plan
        .steps
        .iter()
        .filter(|s| s.seq > last_safe)
        .filter(|s| matches!(s.status.as_str(), "pending" | "running" | "failed" | ""))
        .collect();
    candidates.sort_by_key(|s| s.seq);
    candidates.into_iter().next()
}

/// 任务是否允许 resume（I7：用户取消永不）。
pub fn may_resume(status: &str, cancel_reason: Option<&str>) -> bool {
    if cancel_reason == Some("user") {
        return false;
    }
    matches!(status, "interrupted" | "running" | "failed")
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResumePrepareResult {
    pub task_id: String,
    pub outcome: String, // ready | ask_user | refused | already_done | failed
    pub resume_cursor: Option<i32>,
    pub message: String,
    pub last_safe_seq: i32,
}

/// 查审计 outcome。
pub fn audit_outcome_of(conn: &Connection, tool_call_id: i64) -> rusqlite::Result<Option<String>> {
    conn.query_row(
        "SELECT outcome FROM tool_calls WHERE id = ?1",
        params![tool_call_id],
        |r| r.get(0),
    )
    .optional()
}

/// 同幂等键是否已有成功审计。
pub fn idempotent_success_exists(
    conn: &Connection,
    key: &str,
) -> rusqlite::Result<bool> {
    if key.is_empty() {
        return Ok(false);
    }
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM tool_calls
          WHERE idempotency_key = ?1 AND outcome = 'ok'",
        params![key],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// 构造幂等键（方案 §5.3）。
pub fn make_idempotency_key(task_id: &str, seq: i32, tool_name: &str, canonical_params: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    task_id.hash(&mut h);
    seq.hash(&mut h);
    tool_name.hash(&mut h);
    canonical_params.hash(&mut h);
    format!("{:x}", h.finish())
}

/// 准备恢复：heal → 裁决下一步 → 写回 DB。**不自动执工具**（D1）。
pub fn prepare_resume(conn: &Connection, task_id: &str) -> rusqlite::Result<ResumePrepareResult> {
    let row = conn
        .query_row(
            "SELECT status, cancel_reason, plan_mode FROM tasks WHERE id = ?1",
            params![task_id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, Option<String>>(2)?.unwrap_or_else(|| "none".into()),
                ))
            },
        )
        .optional()?;
    let Some((status, cancel_reason, plan_mode)) = row else {
        return Ok(ResumePrepareResult {
            task_id: task_id.into(),
            outcome: "refused".into(),
            resume_cursor: None,
            message: "任务不存在".into(),
            last_safe_seq: -1,
        });
    };
    if plan_mode != "planned" {
        return Ok(ResumePrepareResult {
            task_id: task_id.into(),
            outcome: "refused".into(),
            resume_cursor: None,
            message: "非计划任务，无法按检查点续跑".into(),
            last_safe_seq: -1,
        });
    }
    if !may_resume(&status, cancel_reason.as_deref()) {
        let msg = if cancel_reason.as_deref() == Some("user") {
            "用户已取消，永不自动恢复"
        } else if status == "done" || status == "canceled" {
            "任务已结束"
        } else {
            "当前状态不可恢复"
        };
        return Ok(ResumePrepareResult {
            task_id: task_id.into(),
            outcome: if status == "done" {
                "already_done".into()
            } else {
                "refused".into()
            },
            resume_cursor: None,
            message: msg.into(),
            last_safe_seq: -1,
        });
    }

    let Some(mut plan) = db::plan_get(conn, task_id)? else {
        return Ok(ResumePrepareResult {
            task_id: task_id.into(),
            outcome: "refused".into(),
            resume_cursor: None,
            message: "无计划 JSON".into(),
            last_safe_seq: -1,
        });
    };

    let audit_lookup = |id: i64| audit_outcome_of(conn, id).ok().flatten();
    if heal_plan_checkpoints(&mut plan, &audit_lookup) {
        db::plan_put(conn, task_id, &plan)?;
    }

    let safe = last_safe_seq(&plan, &audit_lookup);
    let Some(next) = next_resume_step(&plan, safe).cloned() else {
        // 全部安全完成 → 标 done
        conn.execute(
            "UPDATE tasks SET status = 'done', resume_cursor = NULL,
                    safe_checkpoint = ?2 WHERE id = ?1",
            params![task_id, clock::now_iso()],
        )?;
        return Ok(ResumePrepareResult {
            task_id: task_id.into(),
            outcome: "already_done".into(),
            resume_cursor: None,
            message: "检查点已覆盖全部步骤".into(),
            last_safe_seq: safe,
        });
    };

    let effect = EffectClass::from_str_opt(next.effect_class.as_deref());
    let audit = next
        .tool_call_id
        .and_then(|id| audit_outcome_of(conn, id).ok().flatten());
    let idem = next
        .idempotency_key
        .as_deref()
        .map(|k| idempotent_success_exists(conn, k).unwrap_or(false))
        .unwrap_or(false);
    let decision = reentry_decision(&next.status, audit.as_deref(), effect, idem);

    match decision {
        ReentryDecision::Skip => {
            // 标该步 done，递归推进（有界：步数 ≤6）
            if let Some(step) = plan.steps.iter_mut().find(|s| s.seq == next.seq) {
                step.status = "done".into();
            }
            db::plan_put(conn, task_id, &plan)?;
            conn.execute(
                "UPDATE tasks SET resume_cursor = ?2, safe_checkpoint = ?3,
                        status = 'interrupted' WHERE id = ?1",
                params![task_id, next.seq + 1, clock::now_iso()],
            )?;
            // 再准备一次以推进到真正待执行步
            return prepare_resume(conn, task_id);
        }
        ReentryDecision::AskUser => {
            conn.execute(
                "UPDATE tasks SET status = 'interrupted', resume_cursor = ?2,
                        interrupt_kind = COALESCE(interrupt_kind, 'crash')
                  WHERE id = ?1",
                params![task_id, next.seq],
            )?;
            return Ok(ResumePrepareResult {
                task_id: task_id.into(),
                outcome: "ask_user".into(),
                resume_cursor: Some(next.seq),
                message: format!(
                    "步骤 {}（{}）无法证明是否已执行，请选择重试 / 跳过 / 取消",
                    next.seq + 1,
                    next.intent
                ),
                last_safe_seq: safe,
            });
        }
        ReentryDecision::Fail => {
            conn.execute(
                "UPDATE tasks SET status = 'failed', resume_cursor = ?2 WHERE id = ?1",
                params![task_id, next.seq],
            )?;
            return Ok(ResumePrepareResult {
                task_id: task_id.into(),
                outcome: "failed".into(),
                resume_cursor: Some(next.seq),
                message: "写步骤不可安全重放".into(),
                last_safe_seq: safe,
            });
        }
        ReentryDecision::Retry => {
            conn.execute(
                "UPDATE tasks SET status = 'running', resume_cursor = ?2,
                        safe_checkpoint = ?3, interrupt_kind = NULL
                  WHERE id = ?1",
                params![task_id, next.seq, clock::now_iso()],
            )?;
            Ok(ResumePrepareResult {
                task_id: task_id.into(),
                outcome: "ready".into(),
                resume_cursor: Some(next.seq),
                message: format!("已就绪，从步骤 {} 续跑（需云端对话触发执行）", next.seq + 1),
                last_safe_seq: safe,
            })
        }
    }
}

/// 用户对 ask_user 步骤的选择。
pub fn apply_reentry_choice(
    conn: &Connection,
    task_id: &str,
    choice: &str, // retry | skip | cancel
) -> rusqlite::Result<ResumePrepareResult> {
    let cursor: Option<i32> = conn
        .query_row(
            "SELECT resume_cursor FROM tasks WHERE id = ?1",
            params![task_id],
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    let Some(seq) = cursor else {
        return Ok(ResumePrepareResult {
            task_id: task_id.into(),
            outcome: "refused".into(),
            resume_cursor: None,
            message: "无待裁决步骤".into(),
            last_safe_seq: -1,
        });
    };
    match choice {
        "cancel" => {
            db::task_set_cancel_reason(conn, task_id, "user")?;
            conn.execute(
                "UPDATE tasks SET status = 'canceled', resume_cursor = NULL WHERE id = ?1",
                params![task_id],
            )?;
            Ok(ResumePrepareResult {
                task_id: task_id.into(),
                outcome: "refused".into(),
                resume_cursor: None,
                message: "已按用户取消".into(),
                last_safe_seq: -1,
            })
        }
        "skip" => {
            if let Some(mut plan) = db::plan_get(conn, task_id)? {
                if let Some(step) = plan.steps.iter_mut().find(|s| s.seq == seq) {
                    step.status = "skipped".into();
                }
                db::plan_put(conn, task_id, &plan)?;
            }
            conn.execute(
                "UPDATE tasks SET resume_cursor = ?2 WHERE id = ?1",
                params![task_id, seq + 1],
            )?;
            prepare_resume(conn, task_id)
        }
        "retry" => {
            // 用户显式授权重放写步骤
            if let Some(mut plan) = db::plan_get(conn, task_id)? {
                if let Some(step) = plan.steps.iter_mut().find(|s| s.seq == seq) {
                    step.status = "pending".into();
                    // 临时标 read 效应以便 reentry 放行——用户已授权
                    step.effect_class = Some("read".into());
                }
                db::plan_put(conn, task_id, &plan)?;
            }
            conn.execute(
                "UPDATE tasks SET status = 'running', interrupt_kind = NULL WHERE id = ?1",
                params![task_id],
            )?;
            Ok(ResumePrepareResult {
                task_id: task_id.into(),
                outcome: "ready".into(),
                resume_cursor: Some(seq),
                message: "用户授权重试，已标 running".into(),
                last_safe_seq: seq - 1,
            })
        }
        _ => Ok(ResumePrepareResult {
            task_id: task_id.into(),
            outcome: "refused".into(),
            resume_cursor: Some(seq),
            message: "未知选择（retry|skip|cancel）".into(),
            last_safe_seq: -1,
        }),
    }
}

/// 可恢复任务列表（启动标记 / 面板）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResumableTask {
    pub id: String,
    pub goal: String,
    pub status: String,
    pub resume_cursor: Option<i32>,
    pub interrupt_kind: Option<String>,
    pub cancel_reason: Option<String>,
}

pub fn list_resumable(conn: &Connection, limit: i64) -> rusqlite::Result<Vec<ResumableTask>> {
    let mut stmt = conn.prepare(
        "SELECT id, goal, status, resume_cursor, interrupt_kind, cancel_reason
           FROM tasks
          WHERE plan_mode = 'planned'
            AND status IN ('interrupted', 'failed', 'running')
            AND (cancel_reason IS NULL OR cancel_reason != 'user')
          ORDER BY rowid DESC LIMIT ?1",
    )?;
    let rows = stmt.query_map(params![limit.clamp(1, 50)], |r| {
        Ok(ResumableTask {
            id: r.get(0)?,
            goal: r.get(1)?,
            status: r.get(2)?,
            resume_cursor: r.get(3)?,
            interrupt_kind: r.get(4)?,
            cancel_reason: r.get(5)?,
        })
    })?;
    rows.collect()
}

/// 启动：把超时 running 标 interrupted，并写 interrupt_kind（不自动续跑）。
pub fn mark_stale_and_flag_resumable(conn: &Connection) -> rusqlite::Result<usize> {
    let cutoff = (chrono::Utc::now() - chrono::Duration::hours(24)).to_rfc3339();
    conn.execute(
        "UPDATE tasks SET status = 'interrupted',
                interrupt_kind = COALESCE(interrupt_kind, 'crash')
          WHERE status = 'running' AND created_at < ?1",
        params![cutoff],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::db;

    fn step(seq: i32, status: &str, tcid: Option<i64>) -> PlanStep {
        PlanStep {
            seq,
            intent: format!("s{seq}"),
            tool_hint: "fs_read".into(),
            status: status.into(),
            attempts: 1,
            tool_call_id: tcid,
            idempotency_key: None,
            effect_class: None,
        }
    }

    #[test]
    fn checkpoint_requires_done_and_ok_audit() {
        let s = step(0, "done", Some(9));
        assert!(is_checkpoint_complete(&s, Some("ok")));
        assert!(!is_checkpoint_complete(&s, Some("error")));
        assert!(!is_checkpoint_complete(&step(0, "running", Some(9)), Some("ok")));
        assert!(!is_checkpoint_complete(&step(0, "done", None), Some("ok")));
    }

    #[test]
    fn heal_audit_ok_promotes_plan_done() {
        let mut plan = PlanJson {
            version: 1,
            goal: "g".into(),
            steps: vec![step(0, "running", Some(1))],
            replans: 0,
        };
        let lookup = |id: i64| if id == 1 { Some("ok".into()) } else { None };
        assert!(heal_plan_checkpoints(&mut plan, &lookup));
        assert_eq!(plan.steps[0].status, "done");
    }

    #[test]
    fn heal_done_without_audit_demotes() {
        let mut plan = PlanJson {
            version: 1,
            goal: "g".into(),
            steps: vec![step(0, "done", None)],
            replans: 0,
        };
        let lookup = |_id: i64| None;
        assert!(heal_plan_checkpoints(&mut plan, &lookup));
        assert_eq!(plan.steps[0].status, "pending");
    }

    #[test]
    fn reentry_write_without_proof_asks_user() {
        assert_eq!(
            reentry_decision("pending", None, EffectClass::Write, false),
            ReentryDecision::AskUser
        );
        assert_eq!(
            reentry_decision("pending", None, EffectClass::Unknown, false),
            ReentryDecision::AskUser
        );
    }

    #[test]
    fn reentry_read_retries() {
        assert_eq!(
            reentry_decision("pending", None, EffectClass::Read, false),
            ReentryDecision::Retry
        );
    }

    #[test]
    fn reentry_idempotent_key_skips() {
        assert_eq!(
            reentry_decision("pending", None, EffectClass::Write, true),
            ReentryDecision::Skip
        );
    }

    #[test]
    fn user_cancel_never_resumes() {
        assert!(!may_resume("interrupted", Some("user")));
        assert!(may_resume("interrupted", None));
        assert!(may_resume("interrupted", Some("budget")));
        assert!(!may_resume("done", None));
    }

    #[test]
    fn last_safe_and_next_step() {
        let plan = PlanJson {
            version: 1,
            goal: "g".into(),
            steps: vec![
                step(0, "done", Some(1)),
                step(1, "pending", None),
                step(2, "pending", None),
            ],
            replans: 0,
        };
        let lookup = |id: i64| if id == 1 { Some("ok".into()) } else { None };
        let safe = last_safe_seq(&plan, &lookup);
        assert_eq!(safe, 0);
        assert_eq!(next_resume_step(&plan, safe).unwrap().seq, 1);
    }

    #[test]
    fn prepare_resume_db_roundtrip() {
        let conn = Connection::open_in_memory().unwrap();
        db::init_schema_for_test(&conn).unwrap();
        // migrate additive cols for in-memory fresh SCHEMA (v20 in SCHEMA string)
        let id = "t-resume-1";
        conn.execute(
            "INSERT INTO tasks (id, created_at, goal, status, plan_mode, interrupt_kind)
             VALUES (?1, ?2, '读再写', 'interrupted', 'planned', 'crash')",
            params![id, clock::now_iso()],
        )
        .unwrap();
        let plan = PlanJson {
            version: 1,
            goal: "读再写".into(),
            steps: vec![
                {
                    let mut s = step(0, "done", Some(1));
                    s.effect_class = Some("read".into());
                    s
                },
                {
                    let mut s = step(1, "pending", None);
                    s.effect_class = Some("read".into());
                    s.tool_hint = "fs_read".into();
                    s
                },
            ],
            replans: 0,
        };
        db::plan_put(&conn, id, &plan).unwrap();
        conn.execute(
            "INSERT INTO tool_calls (id, ts, tool, action, outcome, task_id)
             VALUES (1, ?1, 'builtin', 'fs_read', 'ok', ?2)",
            params![clock::now_iso(), id],
        )
        .unwrap();

        let r = prepare_resume(&conn, id).unwrap();
        assert_eq!(r.outcome, "ready");
        assert_eq!(r.resume_cursor, Some(1));
        assert_eq!(r.last_safe_seq, 0);

        // 用户取消后拒绝
        db::task_set_cancel_reason(&conn, id, "user").unwrap();
        conn.execute(
            "UPDATE tasks SET status = 'canceled' WHERE id = ?1",
            params![id],
        )
        .unwrap();
        let r2 = prepare_resume(&conn, id).unwrap();
        assert_eq!(r2.outcome, "refused");
    }

    #[test]
    fn write_step_prepare_asks_user() {
        let conn = Connection::open_in_memory().unwrap();
        db::init_schema_for_test(&conn).unwrap();
        let id = "t-write";
        conn.execute(
            "INSERT INTO tasks (id, created_at, goal, status, plan_mode)
             VALUES (?1, ?2, '写文件', 'interrupted', 'planned')",
            params![id, clock::now_iso()],
        )
        .unwrap();
        let plan = PlanJson {
            version: 1,
            goal: "写文件".into(),
            steps: vec![{
                let mut s = step(0, "pending", None);
                s.effect_class = Some("write".into());
                s
            }],
            replans: 0,
        };
        db::plan_put(&conn, id, &plan).unwrap();
        let r = prepare_resume(&conn, id).unwrap();
        assert_eq!(r.outcome, "ask_user");
    }
}

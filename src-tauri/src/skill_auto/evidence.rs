//! Evidence projection (no trace_steps.prompt).

use rusqlite::{params, Connection};

use crate::agent_tools::mask_secrets;

use super::select::load_task_snap;

#[derive(Debug, Clone)]
pub struct Evidence {
    pub goal: String,
    pub final_reply: String,
    pub user_texts: Vec<String>,
    pub reply_texts: Vec<String>,
    pub tool_summaries: Vec<String>,
}

fn looks_like_pii(s: &str) -> bool {
    // 粗筛：11 位手机、18 位身份证、sk-/Bearer 残留（mask 后仍可疑则丢整段）
    let digits: String = s.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.len() >= 11 && s.chars().filter(|c| c.is_ascii_digit()).count() >= 11 {
        // 连续 11 位数字
        let re_phone = digits.contains("1") && digits.len() >= 11;
        if re_phone {
            // 简化：出现 11+ 连续数字
            let mut run = 0;
            for c in s.chars() {
                if c.is_ascii_digit() {
                    run += 1;
                    if run >= 11 {
                        return true;
                    }
                } else {
                    run = 0;
                }
            }
        }
    }
    let lower = s.to_lowercase();
    lower.contains("sk-") || lower.contains("bearer ")
}

fn scrub(s: &str) -> Option<String> {
    let masked = mask_secrets(s);
    if looks_like_pii(&masked) {
        return None;
    }
    Some(masked)
}

/// 白名单投影（§4.3）。**绝不**读取 `trace_steps.prompt`。
pub fn project_evidence(conn: &Connection, task_ids: &[String]) -> rusqlite::Result<Evidence> {
    let mut goal = String::new();
    let mut final_reply = String::new();
    let mut user_texts = Vec::new();
    let mut reply_texts = Vec::new();
    let mut tool_summaries = Vec::new();

    for tid in task_ids {
        if let Some(snap) = load_task_snap(conn, tid)? {
            if goal.is_empty() {
                if let Some(g) = scrub(&snap.goal) {
                    goal = g;
                }
            }
            if final_reply.is_empty() {
                if let Some(r) = snap.final_reply.as_ref().and_then(|x| scrub(x)) {
                    final_reply = r;
                }
            }
            for u in snap.user_texts {
                if let Some(x) = scrub(&u) {
                    user_texts.push(x);
                }
            }
        }

        // tool_calls：params_summary / result_status / tool+action — 不用 detail
        let mut stmt = conn.prepare(
            "SELECT tool, action, COALESCE(params_summary,''), COALESCE(result_status, outcome, '')
             FROM tool_calls WHERE task_id = ?1 ORDER BY id ASC",
        )?;
        let rows = stmt.query_map(params![tid], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })?;
        for row in rows {
            let (tool, action, params_s, status) = row?;
            let line = format!("{tool}:{action} status={status} args={params_s}");
            if let Some(x) = scrub(&line) {
                tool_summaries.push(x);
            }
        }

        // turn_trace reply（按 session）
        if let Some(ref sid) = load_task_snap(conn, tid)?.and_then(|s| s.session_id) {
            let mut tstmt = conn.prepare(
                "SELECT COALESCE(reply_text,'') FROM turn_trace
                 WHERE session_id = ?1 AND reply_text IS NOT NULL
                 ORDER BY id DESC LIMIT 3",
            )?;
            let trows = tstmt.query_map(params![sid], |r| r.get::<_, String>(0))?;
            for tr in trows {
                let r = tr?;
                if let Some(x) = scrub(&r) {
                    reply_texts.push(x);
                }
            }
        }
    }

    Ok(Evidence {
        goal,
        final_reply,
        user_texts,
        reply_texts,
        tool_summaries,
    })
}

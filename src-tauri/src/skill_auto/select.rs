//! Candidate selection from task snaps.

use std::collections::{HashMap, HashSet};

use rusqlite::{params, Connection, OptionalExtension};

use super::EXPLICIT_PHRASES;

#[derive(Debug, Clone)]
pub struct TaskSnap {
    pub id: String,
    pub status: String,
    pub goal: String,
    pub final_reply: Option<String>,
    pub session_id: Option<String>,
    /// 非失败的 tool 调用数。
    pub ok_tools: usize,
    /// 有序 `tool:action` 签名。
    pub chain: Vec<String>,
    pub user_texts: Vec<String>,
    pub crisis: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub task_ids: Vec<String>,
    pub chain_key: String,
    pub explicit: bool,
}

pub(crate) fn chain_key(chain: &[String]) -> String {
    chain.join(">")
}

pub(crate) fn has_explicit(texts: &[String]) -> bool {
    texts.iter().any(|t| EXPLICIT_PHRASES.iter().any(|p| t.contains(p)))
}

/// 纯函数：从快照筛候选（§4.2 + 测试清单 1–2）。
pub fn select_candidates(snaps: &[TaskSnap], consumed: &HashSet<String>) -> Vec<Candidate> {
    let mut by_chain: HashMap<String, Vec<&TaskSnap>> = HashMap::new();
    let mut singles: Vec<Candidate> = Vec::new();

    for s in snaps {
        if consumed.contains(&s.id) || s.crisis {
            continue;
        }
        if s.status != "done" {
            continue;
        }
        let reply_ok = s.final_reply.as_ref().is_some_and(|r| !r.trim().is_empty());
        if !reply_ok {
            continue;
        }
        let explicit = has_explicit(&s.user_texts) || has_explicit(&[s.goal.clone()]);
        if s.ok_tools == 0 && !explicit {
            continue; // 纯闲聊
        }
        if s.ok_tools == 0 && explicit {
            singles.push(Candidate {
                task_ids: vec![s.id.clone()],
                chain_key: format!("explicit:{}", s.id),
                explicit: true,
            });
            continue;
        }
        let key = chain_key(&s.chain);
        by_chain.entry(key).or_default().push(s);
    }

    let mut out = singles;
    for (key, group) in by_chain {
        if group.len() >= 2 {
            out.push(Candidate {
                task_ids: group.iter().map(|s| s.id.clone()).collect(),
                chain_key: key,
                explicit: group.iter().any(|s| {
                    has_explicit(&s.user_texts) || has_explicit(&[s.goal.clone()])
                }),
            });
        } else if let Some(s) = group.first() {
            // 单次成功任务也可成包（§4.2 入选条件一）；聚类只是抬置信。
            out.push(Candidate {
                task_ids: vec![s.id.clone()],
                chain_key: key,
                explicit: has_explicit(&s.user_texts) || has_explicit(&[s.goal.clone()]),
            });
        }
    }
    out
}

pub fn load_task_snap(conn: &Connection, task_id: &str) -> rusqlite::Result<Option<TaskSnap>> {
    let row: Option<(String, String, String, Option<String>, Option<String>)> = conn
        .query_row(
            "SELECT id, status, goal, final_reply, session_id FROM tasks WHERE id = ?1",
            params![task_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()?;
    let Some((id, status, goal, final_reply, session_id)) = row else {
        return Ok(None);
    };

    let mut stmt = conn.prepare(
        "SELECT tool, action, COALESCE(result_status, outcome, '') FROM tool_calls
         WHERE task_id = ?1 ORDER BY id ASC",
    )?;
    let mut ok_tools = 0usize;
    let mut chain = Vec::new();
    let rows = stmt.query_map(params![task_id], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
        ))
    })?;
    for row in rows {
        let (tool, action, status) = row?;
        let fail = status.contains("fail")
            || status.contains("error")
            || status == "denied"
            || status == "timeout";
        if !fail {
            ok_tools += 1;
        }
        chain.push(format!("{tool}:{action}"));
    }

    let mut user_texts = Vec::new();
    let mut crisis = false;
    if let Some(ref sid) = session_id {
        let mut tstmt = conn.prepare(
            "SELECT user_text, kind FROM turn_trace
             WHERE session_id = ?1 AND user_text IS NOT NULL
             ORDER BY id DESC LIMIT 8",
        )?;
        let trows = tstmt.query_map(params![sid], |r| {
            Ok((r.get::<_, Option<String>>(0)?, r.get::<_, String>(1)?))
        })?;
        for tr in trows {
            let (ut, kind) = tr?;
            if kind == "crisis" {
                crisis = true;
            }
            if let Some(u) = ut {
                user_texts.push(u);
            }
        }
    }

    Ok(Some(TaskSnap {
        id,
        status,
        goal,
        final_reply,
        session_id,
        ok_tools,
        chain,
        user_texts,
        crisis,
    }))
}

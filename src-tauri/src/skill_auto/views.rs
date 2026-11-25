//! Pending candidate views for the settings UI.

use std::collections::HashSet;

use rusqlite::Connection;
use serde::Serialize;

use super::select::{chain_key, has_explicit, load_task_snap, select_candidates};
use super::state::{load_state, save_state};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CandidateView {
    pub task_id: String,
    pub goal: String,
    pub status: String,
    pub ok_tools: usize,
    pub explicit: bool,
    pub chain_key: String,
}

pub fn list_pending_views(conn: &Connection) -> rusqlite::Result<Vec<CandidateView>> {
    let st = load_state(conn);
    let consumed: HashSet<String> = st.consumed.iter().cloned().collect();
    let mut out = Vec::new();
    for id in &st.pending {
        if let Some(s) = load_task_snap(conn, id)? {
            let explicit = has_explicit(&s.user_texts) || has_explicit(&[s.goal.clone()]);
            let cands = select_candidates(std::slice::from_ref(&s), &consumed);
            if cands.is_empty() {
                continue;
            }
            out.push(CandidateView {
                task_id: s.id,
                goal: s.goal,
                status: s.status,
                ok_tools: s.ok_tools,
                explicit,
                chain_key: chain_key(&s.chain),
            });
        }
    }
    Ok(out)
}

pub fn dismiss_candidate(conn: &Connection, task_id: &str) -> rusqlite::Result<()> {
    let mut st = load_state(conn);
    st.pending.retain(|x| x != task_id);
    if !st.consumed.iter().any(|x| x == task_id) {
        st.consumed.push(task_id.to_string());
    }
    save_state(conn, &st)
}

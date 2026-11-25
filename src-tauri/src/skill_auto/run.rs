//! Extract / nightly run / M3 prune & use feedback.

use std::collections::HashSet;
use std::path::Path;

use rusqlite::Connection;

use crate::llm::{self, Msg};
use crate::memory::db;
use crate::memory::MemoryStore;
use crate::skill::{self, SkillError};

use super::evidence::project_evidence;
use super::select::{load_task_snap, select_candidates, Candidate};
use super::state::{
    enqueue_candidate, is_feature_enabled, load_meta, load_state, save_meta, save_state,
    AutoMetaEntry,
};
use super::validate::{build_prompt, make_auto_id, validate_generated};
use super::{FAIL_BLACKLIST, GEN_TIMEOUT_SECS, NIGHTLY_CAP, DRAFT_MAX_AGE_DAYS};

pub struct ExtractResult {
    pub id: String,
    pub name: String,
}

/// 对单个候选成包。失败记 failures，不落盘。
pub fn extract_candidate(
    conn: &Connection,
    skills_dir: &Path,
    cand: &Candidate,
    client: &dyn llm::ChatClient,
) -> Result<ExtractResult, String> {
    let existing: HashSet<String> = skill::scan_skills(skills_dir)
        .into_iter()
        .filter_map(|r| r.ok().map(|p| p.id))
        .collect();

    let ev = project_evidence(conn, &cand.task_ids).map_err(|e| e.to_string())?;
    if ev.goal.is_empty() && ev.tool_summaries.is_empty() {
        return Err("empty_evidence".into());
    }

    let prompt = build_prompt(&ev);
    let msgs = vec![
        Msg::system("你只输出技能包草稿，遵守格式。"),
        Msg::user(prompt),
    ];
    let raw = client
        .chat(&msgs, 0.2, GEN_TIMEOUT_SECS)
        .map_err(|e| format!("llm:{e}"))?;
    let gen = validate_generated(&raw).map_err(|e| e.to_string())?;

    let primary = cand
        .task_ids
        .first()
        .cloned()
        .unwrap_or_else(|| "task".into());
    let mut id = make_auto_id(&gen.name, &primary);
    if skill::validate_id(&id).is_err() {
        id = format!("auto-flow-{}", &primary.chars().take(6).collect::<String>());
    }
    // 碰撞：换后缀
    if existing.contains(&id) {
        id = format!("{id}-2");
    }
    if existing.contains(&id) {
        return Err("id_exists".into());
    }

    skill::create_skill_pack(skills_dir, &id, &gen.name, &gen.description, &gen.body)
        .map_err(|e: SkillError| e.map_key().to_string())?;

    let mut meta = load_meta(conn);
    meta.insert(
        id.clone(),
        AutoMetaEntry {
            source_task_ids: cand.task_ids.clone(),
            created_at: crate::memory::clock::now_iso(),
            model: client.label(),
            confidence: if cand.explicit || cand.task_ids.len() >= 2 {
                0.8
            } else {
                0.55
            },
            use_count: 0,
            last_used_at: None,
        },
    );
    let _ = save_meta(conn, &meta);

    let mut st = load_state(conn);
    for tid in &cand.task_ids {
        st.pending.retain(|x| x != tid);
        if !st.consumed.iter().any(|x| x == tid) {
            st.consumed.push(tid.clone());
        }
        st.failures.remove(tid);
    }
    let _ = save_state(conn, &st);

    Ok(ExtractResult {
        id,
        name: gen.name,
    })
}

fn record_failure(conn: &Connection, task_ids: &[String]) {
    let mut st = load_state(conn);
    for tid in task_ids {
        let n = st.failures.entry(tid.clone()).or_insert(0);
        *n += 1;
        if *n >= FAIL_BLACKLIST {
            st.pending.retain(|x| x != tid);
        }
    }
    let _ = save_state(conn, &st);
}

/// 定时/手动入口。总开关关闭 → 立即返回。
pub fn run_if_due(
    store: &MemoryStore,
    skills_dir: &Path,
    force: bool,
) -> Result<Vec<ExtractResult>, String> {
    {
        let conn = store.write_guard().map_err(|e| e.to_string())?;
        if !is_feature_enabled(&conn) {
            return Ok(Vec::new());
        }
        if !force {
            let today = chrono::Local::now().format("%Y-%m-%d").to_string();
            let st = load_state(&conn);
            if st.last_run_date.as_deref() == Some(today.as_str()) {
                return Ok(Vec::new());
            }
        }
    }

    // 读候选（短锁）
    let (cands, pending_ids) = {
        let conn = store.write_guard().map_err(|e| e.to_string())?;
        let st = load_state(&conn);
        let consumed: HashSet<String> = st.consumed.iter().cloned().collect();
        let mut snaps = Vec::new();
        for id in &st.pending {
            if let Ok(Some(s)) = load_task_snap(&conn, id) {
                snaps.push(s);
            }
        }
        let mut cands = select_candidates(&snaps, &consumed);
        // 已存在包的 task 跳过
        let existing: HashSet<String> = skill::scan_skills(skills_dir)
            .into_iter()
            .filter_map(|r| r.ok().map(|p| p.id))
            .collect();
        cands.retain(|c| {
            let id_guess = make_auto_id("flow", c.task_ids.first().map(|s| s.as_str()).unwrap_or("x"));
            !existing.iter().any(|e| e == &id_guess)
        });
        cands.truncate(NIGHTLY_CAP);
        (cands, st.pending.clone())
    };
    let _ = pending_ids;

    if cands.is_empty() {
        if !force {
            let conn = store.write_guard().map_err(|e| e.to_string())?;
            let mut st = load_state(&conn);
            st.last_run_date = Some(chrono::Local::now().format("%Y-%m-%d").to_string());
            let _ = save_state(&conn, &st);
        }
        return Ok(Vec::new());
    }

    // 模型零锁
    let client = llm::client_for(llm::TaskLeg::Background);
    let mut created = Vec::new();
    for cand in &cands {
        let conn = store.write_guard().map_err(|e| e.to_string())?;
        match extract_candidate(&conn, skills_dir, cand, client.as_ref()) {
            Ok(r) => created.push(r),
            Err(e) => {
                log::warn!("skill_auto extract failed: {e}");
                record_failure(&conn, &cand.task_ids);
            }
        }
    }

    {
        let conn = store.write_guard().map_err(|e| e.to_string())?;
        let mut st = load_state(&conn);
        st.last_run_date = Some(chrono::Local::now().format("%Y-%m-%d").to_string());
        let _ = save_state(&conn, &st);
        match gc_stale_drafts(&conn, skills_dir) {
            Ok(ids) if !ids.is_empty() => {
                log::info!("skill_auto gc: removed {} stale drafts", ids.len());
            }
            Err(e) => log::warn!("skill_auto gc: {e}"),
            _ => {}
        }
    }

    Ok(created)
}

/// 手动：指定 task_id 立即成包（无视 nightly 日期，仍受总开关约束可选）。
pub fn extract_now(
    store: &MemoryStore,
    skills_dir: &Path,
    task_id: &str,
    ignore_feature_flag: bool,
) -> Result<ExtractResult, String> {
    {
        let conn = store.write_guard().map_err(|e| e.to_string())?;
        if !ignore_feature_flag && !is_feature_enabled(&conn) {
            return Err("skill_auto_disabled".into());
        }
        let _ = enqueue_candidate(&conn, task_id);
    }
    let client = llm::client_for(llm::TaskLeg::Background);
    let conn = store.write_guard().map_err(|e| e.to_string())?;
    let snap = load_task_snap(&conn, task_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "task_not_found".to_string())?;
    let consumed = HashSet::new();
    let cands = select_candidates(&[snap], &consumed);
    let cand = cands
        .into_iter()
        .next()
        .ok_or_else(|| "not_a_candidate".to_string())?;
    extract_candidate(&conn, skills_dir, &cand, client.as_ref())
}


/// M3：记录自动提取技能被注入 kit 的使用反馈。
pub fn record_skill_use(conn: &Connection, skill_ids: &[String]) {
    if skill_ids.is_empty() {
        return;
    }
    let mut meta = load_meta(conn);
    let now = crate::memory::clock::now_iso();
    let mut dirty = false;
    for id in skill_ids {
        if let Some(e) = meta.get_mut(id) {
            e.use_count = e.use_count.saturating_add(1);
            e.last_used_at = Some(now.clone());
            dirty = true;
        }
    }
    if dirty {
        let _ = save_meta(conn, &meta);
    }
}

fn parse_created_day(iso: &str) -> Option<chrono::NaiveDate> {
    // accept RFC3339 / date prefix
    let day = iso.get(..10)?;
    chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d").ok()
}

/// M3：删除「从未启用」且创建超过 [`DRAFT_MAX_AGE_DAYS`] 的自动草稿包。
/// 返回删除的 skill id 列表。
pub fn gc_stale_drafts(conn: &Connection, skills_dir: &Path) -> Result<Vec<String>, String> {
    let enabled: HashSet<String> = skill::enabled_ids(db::get_setting(conn, "skill_enabled").ok().flatten())
        .into_iter()
        .collect();
    let today = chrono::Local::now().date_naive();
    let meta = load_meta(conn);
    let mut removed = Vec::new();
    for (id, entry) in &meta {
        if enabled.contains(id) {
            continue;
        }
        let Some(created) = parse_created_day(&entry.created_at) else {
            continue;
        };
        let age = (today - created).num_days();
        if age < DRAFT_MAX_AGE_DAYS {
            continue;
        }
        match skill::remove_skill_dir(skills_dir, id) {
            Ok(_) => removed.push(id.clone()),
            Err(e) => log::warn!("skill_auto gc remove {id}: {}", e.code()),
        }
    }
    if !removed.is_empty() {
        let mut meta = load_meta(conn);
        for id in &removed {
            meta.remove(id);
        }
        let _ = save_meta(conn, &meta);
    }
    Ok(removed)
}

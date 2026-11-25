//! Skill pack Tauri commands + views.

use serde::Serialize;

use crate::memory::db;

use super::state::AgentState;
use super::now_conn;

// ---- skills

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillView {
    pub id: String,
    pub name: String,
    pub description: String,
    pub enabled: bool,
    pub valid: bool,
    pub error_code: Option<&'static str>,
    /// plan-skill-auto-extract：来自自动提取的草稿元信息。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_extracted: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_confidence: Option<f64>,
    /// plan-skill-auto-extract M3：启用后被注入对话的次数。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_use_count: Option<u64>,
}

#[tauri::command]
pub fn skill_list(
    store: tauri::State<'_, crate::memory::MemoryStore>,
    agent: tauri::State<'_, AgentState>,
) -> Result<Vec<SkillView>, String> {
    let conn = now_conn(&store)?;
    let enabled = crate::skill::enabled_ids(db::get_setting(&conn, "skill_enabled").ok().flatten());
    let meta = crate::skill_auto::load_meta(&conn);
    Ok(crate::skill::scan_skills(&agent.skills_dir)
        .into_iter()
        .map(|r| match r {
            Ok(p) => {
                let m = meta.get(&p.id);
                SkillView {
                    enabled: enabled.iter().any(|id| id == &p.id),
                    valid: true,
                    error_code: None,
                    auto_extracted: m.map(|_| true),
                    auto_source: m.map(|e| {
                        format!(
                            "来源任务 {} · {}",
                            e.source_task_ids.join(","),
                            e.created_at
                        )
                    }),
                    auto_confidence: m.map(|e| e.confidence),
                    auto_use_count: m.map(|e| e.use_count),
                    id: p.id,
                    name: p.name,
                    description: p.description,
                }
            }
            Err((id, e)) => {
                let err_code = e.code();
                SkillView {
                    id: id.clone(),
                    name: id,
                    description: String::new(),
                    enabled: false,
                    valid: false,
                    error_code: Some(err_code),
                    auto_extracted: None,
                    auto_source: None,
                    auto_confidence: None,
                    auto_use_count: None,
                }
            }
        })
        .collect())
}

#[tauri::command]
pub fn skill_toggle(
    store: tauri::State<'_, crate::memory::MemoryStore>,
    agent: tauri::State<'_, AgentState>,
    id: String,
    enable: bool,
) -> Result<(), String> {
    let conn = now_conn(&store)?;
    // Only a valid pack can be enabled.
    let packs = crate::skill::scan_skills(&agent.skills_dir);
    if enable && !packs.iter().any(|r| matches!(r, Ok(p) if p.id == id)) {
        return Err("skill_invalid".into());
    }
    let mut ids = crate::skill::enabled_ids(db::get_setting(&conn, "skill_enabled").ok().flatten());
    if enable {
        if !ids.iter().any(|x| x == &id) {
            ids.push(id);
        }
    } else {
        ids.retain(|x| x != &id);
    }
    db::set_setting(&conn, "skill_enabled", &serde_json::to_string(&ids).unwrap_or_default())
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn skill_remove(
    store: tauri::State<'_, crate::memory::MemoryStore>,
    agent: tauri::State<'_, AgentState>,
    id: String,
) -> Result<(), String> {
    // Guard + delete live in skill.rs so the traversal rule is unit-tested.
    crate::skill::remove_skill_dir(&agent.skills_dir, &id).map_err(|e| e.code().to_string())?;
    // 清自动提取溯源元信息（若有）。
    {
        let conn = now_conn(&store)?;
        let mut meta = crate::skill_auto::load_meta(&conn);
        if meta.remove(&id).is_some() {
            let _ = crate::skill_auto::save_meta(&conn, &meta);
        }
    }
    let removed = id.clone();
    skill_toggle(store, agent, removed, false)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillRefView {
    pub name: String,
    pub content: String,
}

/// Full pack body for the settings-page preview (skill_read). Engine packs
/// are previewable too — they are just packs with a reserved consumer.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillDetail {
    pub id: String,
    pub name: String,
    pub description: String,
    /// Frontmatter `version:`；缺省为 `None`（预览显示「—」）。
    pub version: Option<String>,
    pub body: String,
    pub refs: Vec<SkillRefView>,
    pub enabled: bool,
    /// body + loaded references, the exact bytes the prompt budget pays for.
    pub size_bytes: usize,
}

#[tauri::command]
pub fn skill_read(
    store: tauri::State<'_, crate::memory::MemoryStore>,
    agent: tauri::State<'_, AgentState>,
    id: String,
) -> Result<SkillDetail, String> {
    crate::skill::check_skill_id(&id).map_err(|e| e.code().to_string())?;
    let conn = now_conn(&store)?;
    let enabled = crate::skill::enabled_ids(db::get_setting(&conn, "skill_enabled").ok().flatten())
        .iter()
        .any(|x| x == &id);
    let dir = agent.skills_dir.join(&id);
    let pack = crate::skill::load_skill(&dir).map_err(|e| e.code().to_string())?;
    Ok(SkillDetail {
        size_bytes: pack.body.len() + pack.refs.iter().map(|(_, c)| c.len()).sum::<usize>(),
        refs: pack
            .refs
            .into_iter()
            .map(|(name, content)| SkillRefView { name, content })
            .collect(),
        enabled,
        id: pack.id,
        name: pack.name,
        description: pack.description,
        version: pack.version,
        body: pack.body,
    })
}

/// Import an external pack directory (path from the system folder picker).
/// Returns the imported id; failures carry a mapping key for humanizeAgentError.
#[tauri::command]
pub fn skill_import(
    agent: tauri::State<'_, AgentState>,
    source: String,
) -> Result<String, String> {
    crate::skill::import_skill_dir(&agent.skills_dir, std::path::Path::new(&source))
        .map_err(|e| e.map_key())
}

/// Create a pack from fields typed in the settings UI.
#[tauri::command]
pub fn skill_create(
    agent: tauri::State<'_, AgentState>,
    id: String,
    name: String,
    description: String,
    body: String,
) -> Result<(), String> {
    crate::skill::create_skill_pack(&agent.skills_dir, &id, &name, &description, &body)
        .map_err(|e| e.map_key())
}

/// 覆盖写已有包（plan-skill-auto-extract M2）。
#[tauri::command]
pub fn skill_update(
    agent: tauri::State<'_, AgentState>,
    id: String,
    name: String,
    description: String,
    body: String,
) -> Result<(), String> {
    crate::skill::update_skill_pack(&agent.skills_dir, &id, &name, &description, &body)
        .map_err(|e| e.map_key())
}

#[tauri::command]
pub fn skill_extract_now(
    store: tauri::State<'_, crate::memory::MemoryStore>,
    agent: tauri::State<'_, AgentState>,
    task_id: String,
) -> Result<serde_json::Value, String> {
    let r = crate::skill_auto::extract_now(&store, &agent.skills_dir, &task_id, true)?;
    Ok(serde_json::json!({ "id": r.id, "name": r.name }))
}

#[tauri::command]
pub fn skill_auto_candidates(
    store: tauri::State<'_, crate::memory::MemoryStore>,
) -> Result<Vec<crate::skill_auto::CandidateView>, String> {
    let conn = now_conn(&store)?;
    crate::skill_auto::list_pending_views(&conn).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn skill_auto_dismiss(
    store: tauri::State<'_, crate::memory::MemoryStore>,
    task_id: String,
) -> Result<(), String> {
    let conn = now_conn(&store)?;
    crate::skill_auto::dismiss_candidate(&conn, &task_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn skill_auto_get_enabled(
    store: tauri::State<'_, crate::memory::MemoryStore>,
) -> Result<bool, String> {
    let conn = now_conn(&store)?;
    Ok(crate::skill_auto::is_feature_enabled(&conn))
}

#[tauri::command]
pub fn skill_auto_set_enabled(
    store: tauri::State<'_, crate::memory::MemoryStore>,
    enabled: bool,
) -> Result<(), String> {
    let conn = now_conn(&store)?;
    crate::skill_auto::set_feature_enabled(&conn, enabled).map_err(|e| e.to_string())
}

/// M3：立即清理超时未启用的自动草稿（默认 14 天）。
#[tauri::command]
pub fn skill_auto_prune_drafts(
    store: tauri::State<'_, crate::memory::MemoryStore>,
    agent: tauri::State<'_, AgentState>,
) -> Result<Vec<String>, String> {
    let conn = now_conn(&store)?;
    crate::skill_auto::gc_stale_drafts(&conn, &agent.skills_dir)
}

/// Load a pack from a remote URL (GitHub repo/tree/raw, GitLab blob, or any
/// https SKILL.md). Network runs on the blocking pool — the fetch is
/// user-initiated and nothing happens until this command is called.
#[tauri::command]
pub async fn skill_load_remote(
    agent: tauri::State<'_, AgentState>,
    url: String,
    id: Option<String>,
) -> Result<String, String> {
    let root = agent.skills_dir.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let (derived, files) = crate::skill_remote::fetch(&url)?;
        let id = id
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or(derived);
        crate::skill::install_pack_files(&root, &id, &files).map_err(|e| e.map_key())?;
        Ok(id)
    })
    .await
    .map_err(|_| "skill_network".to_string())?
}

//! System prompt / environment / budget assembly.

use rusqlite::Connection;

use crate::face::{self, FaceState};
use crate::memory::db;
use crate::persona::prompt::{render, PromptContext};
use crate::persona::Persona;
use crate::relationship;

/// FaceState → the mood wording the persona prompt expects (report §5.1).
pub(crate) fn mood_str(state: FaceState) -> &'static str {
    match state {
        FaceState::Calm => "平静",
        FaceState::Focused => "专注",
        FaceState::Sleepy => "困",
        FaceState::Greeted => "被搭话",
        FaceState::Displeased => "不高兴",
        FaceState::Thinking => "思考",
        FaceState::Proud => "得意",
    }
}

pub(crate) fn is_weekend_today() -> bool {
    use chrono::Datelike;
    matches!(
        chrono::Local::now().date_naive().weekday(),
        chrono::Weekday::Sat | chrono::Weekday::Sun
    )
}

pub(crate) fn system_prompt(
    conn: &Connection,
    card: &Persona,
    hour: u32,
    raining: bool,
) -> rusqlite::Result<String> {
    let boundaries = db::boundaries(conn)?;
    let hour8 = hour as u8;
    let mood = mood_str(face::derive_base_state(hour8, is_weekend_today()));
    // Long-term tone (plan-relationship.md §3.5): 拘谨 / 熟络 / 会顶嘴 / 生疏.
    let relationship_mode = relationship::current_mode(conn);
    // 用户画像（plan-user-profile.md §5）：身份快照每轮固定注入，不走
    // might_recall 门控。传 (中文标签, 值) 对儿——key→label 映射只在
    // profile::label 一处，渲染层零依赖 memory。
    let profile: Vec<(String, String)> = crate::memory::profile::get_all(conn)?
        .into_iter()
        .map(|row| (crate::memory::profile::label(&row.key).to_string(), row.value))
        .collect();
    let profile_pairs: Vec<(&str, &str)> =
        profile.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    Ok(render(
        card,
        &PromptContext {
            mood,
            hour,
            raining,
            user_boundaries: &boundaries,
            relationship_mode: relationship_mode.as_str(),
            user_profile: &profile_pairs,
        },
    ))
}

/// Environment prefix (spec-s3 §2, D6): time / timezone / language before
/// the L1 constitution. Clock goes through `clock::now_local()` so tests can
/// pin it (plan-agent-mcp-skill §1: 环境段时间一律走 pinned 时钟).
pub(crate) fn env_prefix(conn: &Connection) -> String {
    let language = db::get_setting(conn, "language")
        .ok()
        .flatten()
        .unwrap_or_else(|| "简体中文".to_string());
    crate::persona::prompt::env_segment(crate::memory::clock::now_local(), &language)
}

/// 本轮上下文预算（plan-context §6.2 / D5）：云端按模型名取窗口（软预算
/// 32k 仍封顶——窗口大不等于该烧），本地按我们显式设定的 `NUM_CTX` 取。
/// 第二个返回值是「是否云端」，装配层据此决定要不要给工具结果留额度。
pub(crate) fn turn_budget(conn: &Connection) -> (crate::context::Budget, bool) {
    use crate::llm_admin::Provider;
    let cfg = crate::llm_admin::effective(conn);
    match cfg.provider {
        Provider::Cloud => {
            let id = cfg
                .active_id
                .strip_prefix(crate::llm_cloud::CLOUD_PREFIX)
                .unwrap_or(cfg.active_id.as_str());
            let model = crate::llm_cloud::find(id).map(|e| e.model).unwrap_or_default();
            (crate::context::Budget::from_model(&model, None), true)
        }
        Provider::Ollama => (
            crate::context::Budget::new(
                crate::llm::ollama::NUM_CTX,
                crate::context::SOFT_BUDGET_DEFAULT,
            ),
            false,
        ),
    }
}

/// 库里的轮（带 id / 日期）→ 装配层要的纯文本轮。
///
/// 摘要需要 id 与日期，装配只需要两段文本，所以读一次库、用两种视图：
/// 别为此查两遍（多会话之后每轮都要读，翻倍就是浪费）。
pub(crate) fn turns_to_history(rows: &[db::TurnMeta]) -> Vec<crate::persona::eval::Turn> {
    rows.iter()
        .map(|t| crate::persona::eval::Turn {
            user: t.user.clone(),
            assistant: t.assistant.clone(),
        })
        .collect()
}


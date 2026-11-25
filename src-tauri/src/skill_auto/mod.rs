//! 自动提取 Skill（docs/zh/plan-skill-auto-extract.md + v3 M3）。
//!
//! 事件入队（KV）+ 夜间/手动成包；落盘只走 `skill::create_skill_pack`。
//! 严禁投影 `trace_steps.prompt`（防自噬）。SCHEMA 不 bump，状态在 settings KV。
//! M3：草稿超时淘汰 + 启用后使用计数（见 `run::gc_stale_drafts` / `record_skill_use`）。

mod state;
mod select;
mod evidence;
mod validate;
mod run;
mod views;

#[cfg(test)]
mod tests;

pub use state::{
    enqueue_candidate, is_feature_enabled, load_meta, load_state, save_meta, save_state,
    set_feature_enabled, AutoMeta, AutoMetaEntry, AutoState, KEY_ENABLED, KEY_META, KEY_STATE,
};
pub use select::{select_candidates, Candidate, TaskSnap};
pub use evidence::{project_evidence, Evidence};
pub use validate::{make_auto_id, validate_generated, Generated};
pub use run::{
    extract_candidate, extract_now, gc_stale_drafts, record_skill_use, run_if_due, ExtractResult,
};
pub use views::{dismiss_candidate, list_pending_views, CandidateView};

/// 显式「固化流程」短语。
pub const EXPLICIT_PHRASES: &[&str] = &[
    "以后都这样",
    "每次都",
    "记住这个流程",
    "下次照这个来",
    "下次也这样",
    "按这个流程",
    "固定成技能",
    "做成技能",
];

pub(crate) const NIGHTLY_CAP: usize = 2;
pub(crate) const FAIL_BLACKLIST: u32 = 3;
pub(crate) const BODY_HARD_MAX: usize = 4096;
pub(crate) const GEN_TIMEOUT_SECS: u64 = 60;
/// M3：从未启用的自动草稿超过该天数则删除包 + meta。
pub const DRAFT_MAX_AGE_DAYS: i64 = 14;

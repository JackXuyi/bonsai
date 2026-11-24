//! SQLite schema and primitives for the memory layer (split by table domain).
//!
//! Plans: docs/zh/plan-memory.md §3–§4, docs/zh/plan-predict.md §3.4,
//! docs/zh/plan-morph.md §3.2, docs/zh/plan-onboarding.md §3.1,
//! docs/zh/plan-relationship.md §3.3, docs/zh/plan-calendar.md §3.2.
//! Three layers: events (ground truth) → facts (traced assertions) →
//! preferences/boundaries (high-confidence user model).
//!
//! Invariants enforced here (plan §3):
//! - Traceability: facts carry source_event_id; boundaries carry source_fact_id.
//! - No silent rewrite: superseded rows get `invalid_at`, never UPDATEd content.
//! - Forgetting: user-requested deletes are physical and cascade (compliance
//!   Art. 16). `forget_event` / `forget_session` / `forget_all`.
//!
//! Layout（docs/zh/plan-db-split.md，P2-11「只搬家」）：
//! `schema` / `events` / `sessions` / `settings` / `progress` / `audit` / `chat`.
//! 对外路径仍是 `memory::db::*`（本文件 `pub use`）。

mod schema;
mod events;
mod sessions;
mod settings;
mod progress;
mod audit;
mod chat;

#[cfg(test)]
mod tests;

pub use schema::*;
pub use events::*;
pub use sessions::*;
pub use settings::*;
pub use progress::*;
pub use audit::*;
pub use chat::*;

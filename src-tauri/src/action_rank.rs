//! Phase 3 Autonomy · A4 — Action Candidate Ranking
//! （docs/zh/plan-autonomy-phase3/v1 §7）。
//!
//! CandidateGen + Rank 与 Decision 分立：本模块不重定义打扰闸、不调工具。
//! Decision 仍只答「能不能开口」；Ranking 答「先做哪个」。

use serde::Serialize;

/// 需要开口的主动动作种类（静默 job 不进此枚举的 Speak 路径）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionKind {
    Remind,
    PredictVerify,
    DigestNudge,
    JobReport,
    AmbientChat,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionCandidate {
    pub id: String,
    pub kind: ActionKind,
    pub value: f32,
    pub interruption_cost: f32,
    pub dedupe_key: String,
    /// 生成层只读说明书；禁止含密钥。
    pub payload: serde_json::Value,
}

/// 价值 − 打扰 − 同日重复惩罚。
pub fn score(c: &ActionCandidate, already_spoken_keys: &[String]) -> f32 {
    let mut s = c.value - c.interruption_cost;
    if already_spoken_keys.iter().any(|k| k == &c.dedupe_key) {
        s -= 1.0;
    }
    s
}

/// 排序；低于阈值的丢弃。阈值默认 0。
pub fn rank(mut candidates: Vec<ActionCandidate>, already_spoken_keys: &[String]) -> Vec<ActionCandidate> {
    candidates.retain(|c| score(c, already_spoken_keys) > 0.0);
    candidates.sort_by(|a, b| {
        score(b, already_spoken_keys)
            .partial_cmp(&score(a, already_spoken_keys))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    // 同 dedupe_key 只留最高分
    let mut seen = std::collections::HashSet::new();
    candidates.retain(|c| seen.insert(c.dedupe_key.clone()));
    candidates
}

/// 今日观察驱动的轻量候选（AmbientChat）。日历提醒等权威仍在既有 lane；
/// 此处只产「说明书」，不平行实现提醒逻辑。
pub fn generate_ambient(observations: &str, local_date: &str) -> Vec<ActionCandidate> {
    if observations.is_empty() || observations.contains("还没有记录") {
        return Vec::new();
    }
    vec![ActionCandidate {
        id: format!("ambient:{local_date}"),
        kind: ActionKind::AmbientChat,
        value: 0.55,
        interruption_cost: 0.2,
        dedupe_key: format!("ambient:{local_date}"),
        payload: serde_json::json!({ "observations": observations }),
    }]
}

/// 选出本 tick 唯一候选（D6：第一名被 Silent 时本 tick 不降级强行说）。
pub fn pick_top(candidates: Vec<ActionCandidate>, already_spoken_keys: &[String]) -> Option<ActionCandidate> {
    rank(candidates, already_spoken_keys).into_iter().next()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(id: &str, value: f32, cost: f32, key: &str) -> ActionCandidate {
        ActionCandidate {
            id: id.into(),
            kind: ActionKind::AmbientChat,
            value,
            interruption_cost: cost,
            dedupe_key: key.into(),
            payload: serde_json::json!({}),
        }
    }

    #[test]
    fn rank_orders_by_net_value() {
        let ranked = rank(
            vec![c("a", 0.9, 0.8, "a"), c("b", 0.7, 0.1, "b")],
            &[],
        );
        assert_eq!(ranked[0].id, "b");
    }

    #[test]
    fn below_threshold_dropped() {
        assert!(rank(vec![c("x", 0.2, 0.3, "x")], &[]).is_empty());
    }

    #[test]
    fn dedupe_penalty_and_unique_key() {
        let ranked = rank(
            vec![c("a1", 0.9, 0.1, "same"), c("a2", 0.8, 0.1, "same")],
            &[],
        );
        assert_eq!(ranked.len(), 1);
        let penalized = rank(vec![c("a", 0.6, 0.1, "k")], &["k".into()]);
        assert!(penalized.is_empty());
    }

    #[test]
    fn ambient_empty_when_no_obs() {
        assert!(generate_ambient("今天还没有记录到什么动静", "2026-11-24").is_empty());
        assert!(!generate_ambient("剪贴板出现 2 条新内容", "2026-11-24").is_empty());
    }
}

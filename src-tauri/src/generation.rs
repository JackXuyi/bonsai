//! L5 — Generation layer: anti-sycophancy filter (report §5.3 — "having a
//! personality" cannot rely on model conscience; it must be enforced by
//! product mechanics).
//!
//! Scaffold scope: mechanical filters only. The cloud dialogue pipeline
//! (persona card injection → cloud model → filter) is a follow-up milestone.
//!
//! Four enforced rules from the research:
//! 1. Disagreement Budget — at least one substantive disagreement per N turns.
//! 2. No unconditional affirmation — intercept and rewrite opener praise.
//! 3. Specificity — disagreement must point at a concrete argument; hedged
//!    "that's reasonable, but..." forms are banned.
//! 4. Make the cost visible — if going along led to a bad decision, say so.

use serde::Serialize;

/// Openers detected as unconditional affirmation — intercepted and rewritten.
pub const UNCONDITIONAL_AFFIRMATIONS: &[&str] = &[
    "太棒了",
    "绝妙的想法",
    "太完美了",
    "说得真好",
    "Great!",
    "Great idea!",
    "Brilliant!",
    "Brilliant idea",
    "Perfect!",
    "What a fantastic idea!",
];

/// Hedged-disagreement forms — banned: they are performance, not disagreement.
pub const HEDGED_DISAGREEMENTS: &[&str] = &[
    "虽然很有道理，但",
    "虽然你说得对，不过",
    "你说的也有道理，但是",
    "That's a fair point, but",
    "While that's true, maybe consider",
];

#[derive(Debug, Serialize)]
pub struct FilterReport {
    /// Opened with unconditional affirmation → must be rewritten.
    pub unconditional_affirmation: bool,
    /// Disagreement exists but is hedged → must be made specific.
    pub hedged_disagreement: bool,
}

pub fn filter(text: &str) -> FilterReport {
    let lower = text.to_lowercase();
    FilterReport {
        unconditional_affirmation: UNCONDITIONAL_AFFIRMATIONS
            .iter()
            .any(|p| lower.contains(&p.to_lowercase())),
        hedged_disagreement: HEDGED_DISAGREEMENTS
            .iter()
            .any(|p| lower.contains(&p.to_lowercase())),
    }
}

#[tauri::command]
pub fn scaffold_reply(user_text: String) -> FilterReport {
    // Scaffold: runs the mechanical filter over user text to demonstrate the
    // pipeline shape. Real generation injects the persona card and calls the
    // cloud model, then this filter applies to the draft before it ships.
    filter(&user_text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_unconditional_affirmation() {
        assert!(filter("太棒了，就这么干！").unconditional_affirmation);
        assert!(filter("Brilliant idea, let's do it").unconditional_affirmation);
        assert!(!filter("I disagree with your second point, because the data shows X.")
            .unconditional_affirmation);
    }

    #[test]
    fn flags_hedged_disagreement() {
        assert!(filter("虽然很有道理，但也可以考虑……").hedged_disagreement);
        assert!(!filter("我不同意第二点，理由是数据里 X 与 Y 矛盾。").hedged_disagreement);
    }
}

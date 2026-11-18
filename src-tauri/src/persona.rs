//! L3 — Personality layer.
//!
//! Research constraints (report §5.1/§5.2):
//! - Persona is split into three layers: L1 constitution (immutable, version
//!   controlled like code), L2 expression (semi-stable), L3 state (volatile).
//! - The persona card is plain text + state machine and MUST NOT depend on a
//!   specific model: swapping models swaps only expression, not the being.
//! - Every trait must be behaviorally verifiable — no adjective piles.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisagreementPolicy {
    /// e.g. "at least 1 substantive disagreement per 5 interactions"
    pub frequency: String,
    /// e.g. "must point at a specific argument; no hedged disagreement"
    pub form: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Persona {
    pub name: String,
    /// L1 constitution: behaviorally verifiable traits.
    pub traits: Vec<String>,
    /// L1 constitution: values and bottom lines.
    pub values: Vec<String>,
    /// L2 expression: quirks / catchphrases / tone habits.
    pub quirks: Vec<String>,
    /// L1 constitution: things it will not do.
    pub boundaries: Vec<String>,
    /// Anti-sycophancy budget (report §5.3) — the most counterintuitive and
    /// most important section.
    pub disagreement_policy: DisagreementPolicy,
}

/// Load the persona card compiled into the binary. It lives in
/// `resources/persona.json` and is versioned like code.
pub fn load() -> Persona {
    serde_json::from_str(include_str!("../resources/persona.json"))
        .expect("persona.json is invalid — it is versioned like code and must always parse")
}

#[tauri::command]
pub fn get_persona(persona: tauri::State<'_, Persona>) -> Persona {
    persona.inner().clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persona_card_parses() {
        let p = load();
        assert!(!p.name.is_empty());
        assert!(!p.traits.is_empty());
        assert!(!p.boundaries.is_empty());
        assert!(!p.disagreement_policy.frequency.is_empty());
    }
}

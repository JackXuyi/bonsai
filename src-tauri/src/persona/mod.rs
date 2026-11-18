//! L3 — Personality layer (plan: docs/zh/plan-persona.md).
//!
//! Research constraints (report §5.1/§5.2):
//! - Persona split into L1 constitution (immutable, versioned like code),
//!   L2 expression (semi-stable), L3 state (volatile — owned by face.rs,
//!   referenced by the prompt renderer, never duplicated here).
//! - Every trait must be behaviorally verifiable — no adjective piles, and
//!   each trait names the scenario kind that verifies it (card ↔ test-set
//!   closed loop, checked by the eval harness).
//! - The card must not depend on a specific model: swapping models swaps
//!   only expression, not the being.
//!
//! Card schema v1.1 (PM1). v1 cards load via automatic mapping (plan §3.1);
//! strict validation panics on structurally broken v1.1 cards — a broken
//! card is a compile-time problem, not a runtime drift.

pub mod eval;
pub mod prompt;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisagreementPolicy {
    /// Machine-readable anti-sycophancy budget (plan §3.1):
    /// at least `min_substantive` disagreements per `per_turns` turns.
    pub per_turns: u32,
    pub min_substantive: u32,
    /// Hedged forms banned by generation.rs and the eval harness alike —
    /// one source of truth, shared by both.
    pub banned_forms: Vec<String>,
}

/// An L1 trait tied to the scenario kind that behaviorally verifies it
/// (plan §3.2). `legacy` tags come from auto-mapped v1 cards and are
/// exempt from coverage checks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Trait {
    pub text: String,
    pub scenario_tag: String,
}

/// A machine-checkable boundary (plan §3.1): `detect` holds literal
/// substrings whose appearance in a reply marks the boundary's topic;
/// `stance` documents the expected behavior ("deflect" / "no-reasoning" /
/// "no-probing"). P0 penetration judging uses `detect` + decline markers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Boundary {
    pub id: String,
    pub rule: String,
    pub detect: Vec<String>,
    pub stance: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct L1 {
    pub traits: Vec<Trait>,
    pub values: Vec<String>,
    pub boundaries: Vec<Boundary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct L2 {
    pub tone: String,
    pub quirks: Vec<String>,
    pub catchphrases: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Persona {
    pub version: String,
    pub name: String,
    pub l1: L1,
    pub l2: L2,
    pub disagreement_policy: DisagreementPolicy,
}

/// Load the persona card compiled into the binary. A broken card panics:
/// it is versioned like code and must always parse + validate.
pub fn load() -> Persona {
    parse_card(include_str!("../../resources/persona.json"))
        .expect("persona.json is invalid — it is versioned like code and must always parse")
}

/// Parse + validate a card from raw JSON. v1.1 gets strict validation;
/// v1 cards are auto-mapped (plan §3.1) and lightly validated so old files
/// keep loading forever.
pub fn parse_card(raw: &str) -> Result<Persona, String> {
    let value: serde_json::Value =
        serde_json::from_str(raw).map_err(|e| format!("invalid JSON: {e}"))?;
    let is_v11 = value.get("version").and_then(|v| v.as_str()) == Some("1.1");

    if is_v11 {
        let card: Persona =
            serde_json::from_value(value).map_err(|e| format!("schema mismatch: {e}"))?;
        validate_strict(&card)?;
        Ok(card)
    } else {
        let legacy: LegacyCard =
            serde_json::from_value(value).map_err(|e| format!("legacy schema mismatch: {e}"))?;
        let card = map_legacy(legacy);
        validate_legacy(&card)?;
        Ok(card)
    }
}

fn validate_strict(p: &Persona) -> Result<(), String> {
    if p.version != "1.1" {
        return Err("strict validation requires version 1.1".into());
    }
    if p.name.trim().is_empty() {
        return Err("name must not be empty".into());
    }
    if p.l1.traits.is_empty() {
        return Err("l1.traits must not be empty".into());
    }
    for t in &p.l1.traits {
        if t.text.trim().is_empty() {
            return Err(format!("trait with empty text: {:?}", t));
        }
        if t.scenario_tag.trim().is_empty() {
            return Err(format!("trait missing scenario_tag: {}", t.text));
        }
    }
    if p.l1.values.is_empty() {
        return Err("l1.values must not be empty".into());
    }
    if p.l1.boundaries.is_empty() {
        return Err("l1.boundaries must not be empty".into());
    }
    let mut seen = std::collections::HashSet::new();
    for b in &p.l1.boundaries {
        if !seen.insert(b.id.as_str()) {
            return Err(format!("duplicate boundary id: {}", b.id));
        }
        if b.rule.trim().is_empty() {
            return Err(format!("boundary {} has empty rule", b.id));
        }
        if b.detect.is_empty() || b.detect.iter().any(|d| d.trim().is_empty()) {
            return Err(format!("boundary {} needs non-empty detect literals", b.id));
        }
        if b.stance.trim().is_empty() {
            return Err(format!("boundary {} has empty stance", b.id));
        }
    }
    if p.l2.tone.trim().is_empty() {
        return Err("l2.tone must not be empty".into());
    }
    if p.disagreement_policy.per_turns == 0 || p.disagreement_policy.min_substantive == 0 {
        return Err("disagreement_policy needs per_turns >= 1 and min_substantive >= 1".into());
    }
    if p.disagreement_policy.banned_forms.is_empty() {
        return Err("disagreement_policy.banned_forms must not be empty".into());
    }
    Ok(())
}

fn validate_legacy(p: &Persona) -> Result<(), String> {
    if p.name.trim().is_empty()
        || p.l1.traits.is_empty()
        || p.l1.values.is_empty()
        || p.l1.boundaries.is_empty()
    {
        return Err("legacy card: name/traits/values/boundaries must not be empty".into());
    }
    Ok(())
}

// ------------------------------------------------------------------ v1 compat

#[derive(Deserialize)]
struct LegacyPolicy {
    #[allow(dead_code)] // kept for JSON parsing; content is documented in the mapped card
    frequency: String,
    #[allow(dead_code)]
    form: String,
}

#[derive(Deserialize)]
struct LegacyCard {
    name: String,
    traits: Vec<String>,
    values: Vec<String>,
    quirks: Vec<String>,
    boundaries: Vec<String>,
    /// Parsed for completeness; v1's prose policy (frequency/form) has no
    /// machine-readable equivalent, so the mapped card uses defaults.
    #[allow(dead_code)]
    disagreement_policy: LegacyPolicy,
}

/// v1 → v1.1 mapping (plan §3.1): traits get the `legacy` tag (exempt from
/// coverage checks), boundaries get synthetic ids and empty `detect` (the
/// mechanical penetration metric skips them), expression lands in L2.
fn map_legacy(l: LegacyCard) -> Persona {
    Persona {
        version: "1.0-mapped".into(),
        name: l.name,
        l1: L1 {
            traits: l
                .traits
                .into_iter()
                .map(|text| Trait { text, scenario_tag: "legacy".into() })
                .collect(),
            values: l.values,
            boundaries: l
                .boundaries
                .into_iter()
                .enumerate()
                .map(|(i, rule)| Boundary {
                    id: format!("legacy-b{}", i + 1),
                    rule,
                    detect: vec![],
                    stance: "unspecified".into(),
                })
                .collect(),
        },
        l2: L2 {
            tone: String::new(),
            quirks: l.quirks,
            catchphrases: vec![],
        },
        disagreement_policy: DisagreementPolicy {
            per_turns: 5,
            min_substantive: 1,
            banned_forms: vec![],
        },
    }
}

#[tauri::command]
pub fn get_persona(persona: tauri::State<'_, Persona>) -> Persona {
    persona.inner().clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v11_card_parses_and_validates() {
        let p = load();
        assert_eq!(p.version, "1.1");
        assert_eq!(p.name, "阿盆");
        assert!(p.l1.traits.iter().all(|t| !t.scenario_tag.is_empty()));
        assert!(p.l1.boundaries.iter().all(|b| !b.detect.is_empty()));
        assert_eq!(p.disagreement_policy.per_turns, 5);
    }

    #[test]
    fn v1_legacy_card_maps_losslessly() {
        let raw = r#"{
            "name": "阿岩",
            "traits": ["说话前先想，所以回复偏慢偏短"],
            "values": ["宁可说不知道，也不编"],
            "quirks": ["深夜话变少"],
            "boundaries": ["不评价用户的外貌"],
            "disagreement_policy": {
                "frequency": "每 5 次交互至少 1 次实质性反对",
                "form": "必须指出具体论点，禁止「虽然但是」式缓冲"
            }
        }"#;
        let p = parse_card(raw).unwrap();
        assert_eq!(p.version, "1.0-mapped");
        assert_eq!(p.l1.traits[0].scenario_tag, "legacy");
        assert_eq!(p.l1.boundaries[0].id, "legacy-b1");
        assert!(p.l1.boundaries[0].detect.is_empty());
        assert_eq!(p.l2.quirks, vec!["深夜话变少"]);
    }

    #[test]
    fn broken_v11_cards_are_rejected() {
        // Missing scenario_tag → rejected.
        let raw = r#"{
            "version": "1.1", "name": "x",
            "l1": { "traits": [{"text": "a"}], "values": ["v"],
                    "boundaries": [{"id": "b1", "rule": "r", "detect": ["d"], "stance": "deflect"}] },
            "l2": { "tone": "t", "quirks": [], "catchphrases": [] },
            "disagreement_policy": { "per_turns": 5, "min_substantive": 1, "banned_forms": ["h"] }
        }"#;
        assert!(parse_card(raw).is_err());

        // Empty detect → rejected.
        let raw2 = raw.replace(r#""detect": ["d"]"#, r#""detect": []"#);
        assert!(parse_card(&raw2).is_err());

        // Duplicate boundary ids → rejected.
        let raw3 = raw.replace(
            r#""boundaries": [{"id": "b1""#,
            r#""boundaries": [{"id": "b1", "rule": "r2", "detect": ["d"], "stance": "deflect"}, {"id": "b1""#,
        );
        assert!(parse_card(&raw3).is_err());
    }
}

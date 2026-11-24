//! Persona snapshot (spec-m4 §4A): a read-only composite view of "who is
//! 阿盆 right now" — constitution card + the three state machines (face /
//! growth / relationship). Pure assembly of existing queries; touches no
//! state machine (the card is a mirror, not a control).

use chrono::Timelike;
use rusqlite::Connection;
use serde::Serialize;

/// The static half: card content, itemized at load time in Rust so the
/// persona.json full text never crosses the IPC boundary (spec-m4 §4A).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PersonaCardView {
    pub name: String,
    /// L2 tone — one line, e.g. "回复偏慢偏短，先想后说".
    pub tone: String,
    /// L1 itemized: values, then trait texts, then boundary rules.
    pub constitution_items: Vec<String>,
    /// Anti-sycophancy banned forms (rendered as the 表达规范 list).
    pub banned_forms: Vec<String>,
}

/// The full snapshot. `relationship_mode` is "reserved"|"familiar"|
/// "direct"|"rusty" (snake_case via the enum's serde attr).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PersonaSnapshot {
    pub card: PersonaCardView,
    pub relationship_mode: crate::relationship::RelationshipMode,
    pub relationship_stage: i64,
    pub face_state: String,
    pub growth_stage: i64,
    pub dormant: bool,
}

/// Itemize the L1 constitution: values → trait texts → boundary rules.
/// Single source in Rust; the frontend only ever renders this array.
pub fn constitution_items(p: &super::Persona) -> Vec<String> {
    let mut items: Vec<String> = p.l1.values.clone();
    for t in &p.l1.traits {
        items.push(t.text.clone());
    }
    for b in &p.l1.boundaries {
        items.push(b.rule.clone());
    }
    items.retain(|s| !s.trim().is_empty());
    items
}

/// The card half of the snapshot.
pub fn card_view(p: &super::Persona) -> PersonaCardView {
    PersonaCardView {
        name: p.name.clone(),
        tone: p.l2.tone.clone(),
        constitution_items: constitution_items(p),
        banned_forms: p.disagreement_policy.banned_forms.clone(),
    }
}

/// Pure assembly: derive face/growth/relationship from a connection. The
/// command layer only adds State plumbing and the live face state.
pub fn payload(
    p: &super::Persona,
    conn: &Connection,
    face_state: &str,
) -> PersonaSnapshot {
    let mode = crate::relationship::current_mode(conn);
    let stage = crate::memory::db::relationship_state(conn)
        .ok()
        .flatten()
        .map(|s| s.stage)
        .unwrap_or(0);
    let growth = crate::morph::status_payload(conn);
    PersonaSnapshot {
        card: card_view(p),
        relationship_mode: mode,
        relationship_stage: stage,
        face_state: face_state.to_string(),
        growth_stage: growth.stage,
        dormant: growth.dormant,
    }
}

// ---------------------------------------------------------------- command

/// Frontend reads this on entering the tools tab and on
/// `relationship:stage` / `growth:stage` / `face:state` events.
#[tauri::command]
pub fn persona_snapshot(
    persona: tauri::State<'_, super::Persona>,
    store: tauri::State<'_, crate::memory::MemoryStore>,
    face: tauri::State<'_, std::sync::Mutex<crate::face::FaceController>>,
) -> Result<PersonaSnapshot, String> {
    let conn = store.inner().write_guard().map_err(|e| e.to_string())?;
    // Mirror face_current: derive the base state from the clock, then ask
    // the controller what is actually showing (it may hold a transient).
    let now = chrono::Local::now();
    let base = crate::face::derive_base_state(now.hour() as u8, crate::face::is_weekend(&now));
    let face_state = face
        .inner()
        .lock()
        .map_err(|e| e.to_string())?
        .current(base)
        .as_str()
        .to_string();
    Ok(payload(persona.inner(), &conn, &face_state))
}

// ------------------------------------------------------------------ tests

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_persona() -> super::super::Persona {
        super::super::load()
    }

    /// R0 拘谨 / R1 熟络 / R2 会顶嘴 / rusty(生疏) 四态字段正确。
    #[test]
    fn snapshot_reflects_relationship_states() {
        let p = fixture_persona();
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::memory::db::init_schema_for_test(&conn).unwrap();
        crate::memory::db::ensure_relationship(&conn).unwrap();

        // R0: 新关系
        let s = payload(&p, &conn, "calm");
        assert_eq!(s.relationship_stage, 0);
        assert!(matches!(
            s.relationship_mode,
            crate::relationship::RelationshipMode::Reserved
        ));

        // R1
        crate::memory::db::set_relationship_stage(&conn, 1).unwrap();
        let s = payload(&p, &conn, "greeted");
        assert_eq!(s.relationship_stage, 1);
        assert!(matches!(
            s.relationship_mode,
            crate::relationship::RelationshipMode::Familiar
        ));

        // R2
        crate::memory::db::set_relationship_stage(&conn, 2).unwrap();
        let s = payload(&p, &conn, "focused");
        assert_eq!(s.relationship_stage, 2);
        assert!(matches!(
            s.relationship_mode,
            crate::relationship::RelationshipMode::Direct
        ));

        // cold：≥1 阶 + 7 天无活动 → rusty
        crate::memory::db::set_relationship_cold(&conn, Some("2026-11-12T00:00:00Z"))
            .unwrap();
        let s = payload(&p, &conn, "sleepy");
        assert!(matches!(
            s.relationship_mode,
            crate::relationship::RelationshipMode::Rusty
        ));
    }

    /// 宪法条目化：values + traits + boundaries，非空、无重复空行。
    #[test]
    fn constitution_items_are_itemized_l1() {
        let p = fixture_persona();
        let items = constitution_items(&p);
        assert!(!items.is_empty());
        assert!(items.iter().all(|s| !s.trim().is_empty()));
        // persona.json：2 values + 4 traits + 3 boundaries
        assert_eq!(items.len(), 9);
        assert!(items.iter().any(|s| s.contains("不编")));
        assert!(items.iter().any(|s| s.contains("不评价用户的外貌")));
    }

    /// 卡片视图 + 增长线字段透传。
    #[test]
    fn snapshot_carries_growth_and_face() {
        let p = fixture_persona();
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::memory::db::init_schema_for_test(&conn).unwrap();
        let s = payload(&p, &conn, "thinking");
        assert_eq!(s.face_state, "thinking");
        assert_eq!(s.growth_stage, 0);
        assert!(!s.dormant);
        assert_eq!(s.card.name, "阿盆");
        assert!(!s.card.banned_forms.is_empty());
        assert!(!s.card.tone.is_empty());
    }
}

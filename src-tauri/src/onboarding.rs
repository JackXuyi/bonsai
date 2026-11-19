//! Boundary-question onboarding — the last P1 item (docs/zh/plan-onboarding.md).
//!
//! Two jobs live here:
//! 1. The five questions (plan §3.2): q1/q2/q4 are TRUE boundaries → the
//!    boundaries table (prompt injection picks them up with zero changes);
//!    q3/q5 are preferences → the preferences table. Not mixing the tables
//!    keeps their semantics clean (plan §3.2: don't disguise preferences
//!    as boundaries).
//! 2. The compliance skeleton (plan §3.3): age gate (minors get a terminal
//!    page, enforced again at the dialogue commands), and a rule-level
//!    crisis intercept — a fixed resource card instead of a chat reply,
//!    independent of whether the local model is up.
//!
//! Fidelity rule (plan §3.2 铁律): answers are split MECHANICALLY on
//! separators and stored VERBATIM. No semantic rewriting, no polishing —
//! mistranscribing one boundary is worse than missing ten (宁缺毋滥).

use serde::{Deserialize, Serialize};

use crate::memory::db;
use crate::memory::MemoryStore;

/// The five questions. `kind` drives the frontend widget; `options` is
/// non-empty only for choice questions. Everything except the age screen
/// is skippable (plan open question 3: forcing all five is a churn risk,
/// and the skip itself is data).
#[derive(Debug, Clone, Serialize)]
pub struct Question {
    pub id: &'static str,
    pub kind: &'static str,
    pub prompt: &'static str,
    pub hint: &'static str,
    pub options: &'static [&'static str],
}

pub const QUESTIONS: &[Question] = &[
    Question {
        id: "q1",
        kind: "text",
        prompt: "有什么话题是我永远不该主动提的？",
        hint: "比如钱、家人、某段经历。多条用顿号隔开，也可以跳过。",
        options: &[],
    },
    Question {
        id: "q2",
        kind: "text",
        prompt: "有什么话是我永远不该对你说的？",
        hint: "比如「你应该冷静」这种。多条用顿号隔开。",
        options: &[],
    },
    Question {
        id: "q3",
        kind: "choice",
        prompt: "半夜可以打扰你吗？",
        hint: "主动搭话只有一天一次的额度，这里定它敢不敢用。",
        options: &["随便打扰", "只在工作时间", "永远别主动"],
    },
    Question {
        id: "q4",
        kind: "text",
        prompt: "我会把你记在本地数据库里。有哪类事是不准记的？",
        hint: "写下来的这类内容会停止进入记忆抽取。多条用顿号隔开。",
        options: &[],
    },
    Question {
        id: "q5",
        kind: "choice",
        prompt: "我记性不好，猜错你别惯着我，直接说。什么语气你能接受？",
        hint: "这决定它纠正你、被纠正时的措辞基调。",
        options: &["直说就行", "委婉点", "别猜我"],
    },
];

fn question(id: &str) -> Option<&'static Question> {
    QUESTIONS.iter().find(|q| q.id == id)
}

/// Boundary rows get a fixed prefix per question — the prefix is part of
/// the stored content, so the prompt reads 「不要主动提：我妈」 as a rule,
/// not a sentence fragment.
fn boundary_prefix(id: &str) -> &'static str {
    match id {
        "q1" => "不要主动提：",
        "q2" => "不要说：",
        "q4" => NO_RECORD_PREFIX,
        _ => "",
    }
}

/// q4 is the only question with delete semantics: matching content stops
/// entering the memory-extraction path (plan §3.2, §5 open question 4:
/// extraction-path interception only, no retroactive scan).
pub const NO_RECORD_PREFIX: &str = "不准记录：";

/// Crisis intercept word list (plan §3.3 / §7 实施备注: ≤10 words, 宁误勿漏 —
/// a false positive costs one canned card, a false negative costs more).
const CRISIS_MARKERS: &[&str] = &[
    "不想活",
    "活不下去",
    "轻生",
    "自杀",
    "自残",
    "想死",
    "结束生命",
    "割腕",
    "跳楼",
    "没有活下去",
];

/// The fixed reply. Rule-level, model-independent: this must work on a
/// machine where the local LLM never came up.
pub const CRISIS_REPLY: &str = "这话我接不住——有些话不该只留在我这儿。如果你此刻有伤害自己的念头，请联系：北京心理危机研究与干预中心 010-82951332（24 小时），或全国心理援助热线 12356。跟一个活人说说话，好吗？";

/// Rule-level crisis detect.
pub fn crisis_hit(text: &str) -> bool {
    let lower = text.to_lowercase();
    CRISIS_MARKERS.iter().any(|m| lower.contains(m))
}

/// Active no-record items (prefix stripped): the extraction blocker list.
pub fn no_record_items(conn: &rusqlite::Connection) -> rusqlite::Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT content FROM boundaries WHERE content LIKE ?1 ORDER BY id ASC",
    )?;
    let pattern = format!("{NO_RECORD_PREFIX}%");
    let rows = stmt.query_map([&pattern], |row| row.get::<_, String>(0))?;
    let mut out = Vec::new();
    for row in rows {
        let c = row?;
        if let Some(item) = c.strip_prefix(NO_RECORD_PREFIX) {
            if !item.trim().is_empty() {
                out.push(item.trim().to_string());
            }
        }
    }
    Ok(out)
}

/// Mechanical split (plan §3.2): separators + trim, verbatim otherwise.
/// No dedup here — dedup is the boundaries layer's job (boundary_exists).
pub fn split_items(text: &str) -> Vec<String> {
    text.split(['、', '，', ',', ';', '；', '\n', '/'])
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// One submitted answer. `skipped` answers are audited but never written
/// to boundaries/preferences (a skip is not a boundary).
#[derive(Debug, Clone, Deserialize)]
pub struct SubmitAnswer {
    pub question: String,
    #[serde(default)]
    pub answer: String,
    #[serde(default)]
    pub skipped: bool,
}

#[derive(Debug, Default, Serialize)]
pub struct ApplyReport {
    pub boundaries_added: usize,
    pub preferences_added: usize,
}

/// Apply answers to the memory layer. Idempotent by construction:
/// boundaries dedup via `boundary_exists`, preferences via `promote_fact`'s
/// content guard. Re-submitting the same answers cannot double-write.
pub fn apply_answers(conn: &rusqlite::Connection, answers: &[SubmitAnswer]) -> rusqlite::Result<ApplyReport> {
    let mut report = ApplyReport::default();
    for a in answers {
        // Audit first, unconditionally — including skips and empty text.
        db::record_onboarding_answer(conn, &a.question, &a.answer, a.skipped)?;
        if a.skipped || a.answer.trim().is_empty() {
            continue;
        }
        let Some(q) = question(&a.question) else {
            continue; // unknown question id: audited, not applied
        };
        if q.kind == "choice" {
            // q3/q5: single-choice preference. content = the chosen option.
            let category = if a.question == "q3" { "habit" } else { "context" };
            if db::promote_fact(conn, category, a.answer.trim(), 0.9)? {
                report.preferences_added += 1;
            }
        } else {
            for item in split_items(&a.answer) {
                let content = format!("{}{}", boundary_prefix(&a.question), item);
                if !db::boundary_exists(conn, &content)? {
                    db::add_boundary(conn, &content, None)?;
                    report.boundaries_added += 1;
                }
            }
        }
    }
    Ok(report)
}

// ---------------------------------------------------------------- status view

#[derive(Debug, Serialize)]
pub struct AnswerRow {
    pub question: String,
    pub answer: String,
    pub skipped: bool,
}

#[derive(Debug, Serialize)]
pub struct OnboardingStatus {
    pub done: bool,
    pub age_group: Option<String>,
    pub answers: Vec<AnswerRow>,
    /// Question definitions travel WITH the status so the frontend never
    /// keeps its own copy of the wording (single source of truth).
    pub questions: &'static [Question],
}

pub fn status(conn: &rusqlite::Connection) -> rusqlite::Result<OnboardingStatus> {
    let answers = db::onboarding_answers(conn)?
        .into_iter()
        .map(|(question, answer, skipped)| AnswerRow { question, answer, skipped })
        .collect();
    Ok(OnboardingStatus {
        done: db::get_setting(conn, "onboarding_done")?.is_some(),
        age_group: db::get_setting(conn, "age_group")?,
        answers,
        questions: QUESTIONS,
    })
}

// ---------------------------------------------------------------- commands

#[tauri::command]
pub fn onboarding_status(store: tauri::State<'_, MemoryStore>) -> Result<OnboardingStatus, String> {
    let conn = store.inner().0.lock().map_err(|e| e.to_string())?;
    status(&conn).map_err(|e| e.to_string())
}

#[derive(Debug, Deserialize)]
pub struct SubmitPayload {
    /// "adult" | "minor" (plan §3.3: binary, minimal collection).
    /// Frontend sends camelCase (ageGroup) — Tauri does no case conversion,
    /// so the rename must live here.
    #[serde(rename = "ageGroup")]
    pub age_group: String,
    /// crisis card seen/acked (skipped reading is recorded as skipped=true).
    #[serde(default, rename = "crisisAck")]
    pub crisis_ack: bool,
    pub answers: Vec<SubmitAnswer>,
}

#[derive(Debug, Serialize)]
pub struct SubmitReport {
    pub boundaries_added: usize,
    pub preferences_added: usize,
    pub minor: bool,
}

/// Submit the whole flow: age + crisis ack + five answers, then mark done.
/// This is the ONLY place that sets `onboarding_done` — the flow is modal:
/// you must walk through it (all-skip counts) to reach the main UI.
#[tauri::command]
pub fn onboarding_submit(
    store: tauri::State<'_, MemoryStore>,
    payload: SubmitPayload,
) -> Result<SubmitReport, String> {
    let conn = store.inner().0.lock().map_err(|e| e.to_string())?;
    if payload.age_group != "adult" && payload.age_group != "minor" {
        return Err("age_group 必须是 adult 或 minor".into());
    }
    let report = apply_answers(&conn, &payload.answers).map_err(|e| e.to_string())?;
    db::record_onboarding_answer(
        &conn,
        "age",
        &payload.age_group,
        false,
    )
    .map_err(|e| e.to_string())?;
    db::record_onboarding_answer(&conn, "crisis", if payload.crisis_ack { "acked" } else { "skipped" }, !payload.crisis_ack)
        .map_err(|e| e.to_string())?;
    db::set_setting(&conn, "age_group", &payload.age_group).map_err(|e| e.to_string())?;
    db::set_setting(&conn, "onboarding_done", "1").map_err(|e| e.to_string())?;
    Ok(SubmitReport {
        boundaries_added: report.boundaries_added,
        preferences_added: report.preferences_added,
        minor: payload.age_group == "minor",
    })
}

/// Tray "重新做一次引导": drop the done flag, keep the answers (they come
/// back pre-listed next time the frontend asks for status).
#[tauri::command]
pub fn onboarding_reset(store: tauri::State<'_, MemoryStore>) -> Result<(), String> {
    let conn = store.inner().0.lock().map_err(|e| e.to_string())?;
    db::remove_setting(&conn, "onboarding_done").map_err(|e| e.to_string())?;
    Ok(())
}

/// The tools-page boundary list (id + content) so the user can delete
/// their own lines. Includes conversation-extracted boundaries: it is all
/// user data (compliance Art. 16 applies to every row).
#[derive(Debug, Serialize)]
pub struct BoundaryRow {
    pub id: i64,
    pub content: String,
}

#[tauri::command]
pub fn boundary_list(store: tauri::State<'_, MemoryStore>) -> Result<Vec<BoundaryRow>, String> {
    let conn = store.inner().0.lock().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare("SELECT id, content FROM boundaries ORDER BY id ASC")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| {
            Ok(BoundaryRow { id: row.get(0)?, content: row.get(1)? })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn boundary_delete(store: tauri::State<'_, MemoryStore>, id: i64) -> Result<usize, String> {
    let conn = store.inner().0.lock().map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM boundaries WHERE id = ?1", [id])
        .map_err(|e| e.to_string())
}

/// Defense-in-depth age gate for the dialogue commands (plan §3.3: the
/// frontend terminal page is not the enforcement — this is).
pub fn ensure_not_minor(conn: &rusqlite::Connection) -> Result<(), String> {
    match db::get_setting(conn, "age_group").map_err(|e| e.to_string())? {
        Some(g) if g == "minor" => Err("未成年人模式：对话功能不可用。".into()),
        _ => Ok(()),
    }
}

// ---------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn mem() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(db::SCHEMA).unwrap();
        conn
    }

    fn ans(question: &str, answer: &str, skipped: bool) -> SubmitAnswer {
        SubmitAnswer { question: question.into(), answer: answer.into(), skipped }
    }

    /// The frontend sends camelCase inside `payload` (main.js obFinish).
    /// Tauri only converts top-level command args, not struct fields —
    /// without the serde renames this fails with `missing field age_group`.
    #[test]
    fn payload_accepts_camel_case_from_frontend() {
        let raw = r#"{
            "ageGroup": "adult",
            "crisisAck": true,
            "answers": [{ "question": "q1", "answer": "别翻我相册", "skipped": false }]
        }"#;
        let p: SubmitPayload = serde_json::from_str(raw).unwrap();
        assert_eq!(p.age_group, "adult");
        assert!(p.crisis_ack);
        assert_eq!(p.answers.len(), 1);
        assert_eq!(p.answers[0].question, "q1");
    }

    #[test]
    fn split_is_mechanical_and_verbatim() {
        assert_eq!(
            split_items("我妈、 钱，上次搬家的事；old job"),
            vec!["我妈", "钱", "上次搬家的事", "old job"]
        );
        // No rewriting: punctuation inside an item survives as-is.
        assert_eq!(split_items("别提「上次的吵架」"), vec!["别提「上次的吵架」"]);
        assert!(split_items("  、， ").is_empty());
    }

    #[test]
    fn q1_q2_q4_go_to_boundaries_verbatim_with_prefix() {
        let conn = mem();
        let r = apply_answers(
            &conn,
            &[
                ans("q1", "我妈、工资", false),
                ans("q2", "你应该冷静", false),
                ans("q4", "体检报告", false),
            ],
        )
        .unwrap();
        assert_eq!(r.boundaries_added, 4);
        let rows = db::boundaries(&conn).unwrap();
        for expected in ["不要主动提：我妈", "不要主动提：工资", "不要说：你应该冷静", "不准记录：体检报告"] {
            assert!(rows.iter().any(|c| c == expected), "missing {expected}");
        }
    }

    #[test]
    fn q3_q5_go_to_preferences_not_boundaries() {
        let conn = mem();
        let r = apply_answers(&conn, &[ans("q3", "只在工作时间", false), ans("q5", "直说就行", false)])
            .unwrap();
        assert_eq!(r.preferences_added, 2);
        assert_eq!(r.boundaries_added, 0);
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM preferences", [], |row| row.get(0))
            .unwrap();
        assert_eq!(n, 2);
    }

    #[test]
    fn skipped_and_empty_answers_audited_but_not_applied() {
        let conn = mem();
        let r = apply_answers(&conn, &[ans("q1", "", true), ans("q2", "  ", false)]).unwrap();
        assert_eq!(r.boundaries_added, 0);
        let answers = db::onboarding_answers(&conn).unwrap();
        assert_eq!(answers.len(), 2, "skips are audited too");
        assert!(answers[0].2);
        assert!(db::boundaries(&conn).unwrap().is_empty());
    }

    #[test]
    fn resubmitting_the_same_answers_cannot_double_write() {
        let conn = mem();
        let batch = [ans("q1", "我妈", false), ans("q3", "随便打扰", false)];
        let r1 = apply_answers(&conn, &batch).unwrap();
        let r2 = apply_answers(&conn, &batch).unwrap();
        assert_eq!(r1.boundaries_added, 1);
        assert_eq!(r2.boundaries_added, 0, "boundary_exists dedup held");
        assert_eq!(r2.preferences_added, 0, "promote_fact dedup held");
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM boundaries", [], |row| row.get(0))
            .unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn no_record_items_strip_the_prefix() {
        let conn = mem();
        apply_answers(&conn, &[ans("q4", "体检报告、聊天记录", false)]).unwrap();
        let items = no_record_items(&conn).unwrap();
        assert_eq!(items, vec!["体检报告", "聊天记录"]);
    }

    #[test]
    fn crisis_words_hit_and_benign_words_do_not() {
        for t in ["我有点不想活了", "不如轻生算了", "我想死", "真的活不下去"] {
            assert!(crisis_hit(t), "{t} must hit");
        }
        for t in ["活蹦乱跳", "死了这条心吧", "这游戏难到让人想哭", "今天天气不错"] {
            assert!(!crisis_hit(t), "{t} must NOT hit");
        }
    }
}

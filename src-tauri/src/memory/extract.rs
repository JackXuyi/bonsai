//! Fact extraction (plan-memory.md §5.2–§5.3).
//!
//! `FactExtractor` decouples the consolidate pipeline from the model layer:
//! - `RuleExtractor` — P0 default; deterministic, offline, privacy-mode safe.
//! - `CloudExtractor` — lands with the cloud dialogue pipeline (plan M5);
//!   same trait, structured JSON in / out.
//!
//! Per the plan's open-question recommendation, stage ① (session summaries)
//! is NOT built in P0, and `importance` stays a column without logic.

use super::db::{ActiveFact, EventRow};

/// Four preference categories (report §4.1, plan §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    Like,
    Habit,
    Context,
    Boundary,
}

impl Category {
    pub fn as_str(&self) -> &'static str {
        match self {
            Category::Like => "like",
            Category::Habit => "habit",
            Category::Context => "context",
            Category::Boundary => "boundary",
        }
    }
}

/// One extracted candidate fact, fed into the merge / contradiction stages.
#[derive(Debug, Clone)]
pub struct CandidateFact {
    pub content: String,
    pub category: Category,
    pub confidence: f64,
    /// Event that produced this candidate (traceability invariant).
    pub source_event_id: Option<i64>,
    /// Correction marker: the pipeline resolves WHICH active fact this
    /// supersedes at apply time (so same-run facts count — plan §5.1 ④).
    pub is_correction: bool,
}

/// What an extractor may look at. `active_facts` is provided so rule-based
/// duplicate detection and correction detection stay deterministic.
pub struct SessionDigest<'a> {
    pub user_utterances: &'a [EventRow],
    pub active_facts: &'a [ActiveFact],
}

pub trait FactExtractor: Send {
    fn extract(&self, digest: &SessionDigest) -> Vec<CandidateFact>;
}

// ------------------------------------------------------------------ rules

/// Normalization for exact-match merging (plan §5.3): lowercase, strip
/// punctuation and whitespace. Replaced by semantic similarity once the
/// vector leg lands.
pub fn normalize(text: &str) -> String {
    text.chars()
        .filter(|c| {
            !c.is_whitespace()
                && !c.is_ascii_punctuation()
                && !['，', '。', '！', '？', '、', '：', '；', '「', '」', '…', '·', '“', '”', '‘', '’']
                    .contains(c)
        })
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// Direct-statement prefixes: (prefix, category, confidence).
/// Kept to fixed patterns on purpose — the plan's risk table says P0 must
/// prefer false negatives over false positives (宁漏勿误).
const DIRECT_PATTERNS: &[(&str, Category, f64)] = &[
    ("我喜欢", Category::Like, 0.7),
    ("我讨厌", Category::Like, 0.7),
    ("我不喜欢", Category::Like, 0.7),
    ("我爱", Category::Like, 0.7),
    ("我每天", Category::Habit, 0.7),
    ("我习惯", Category::Habit, 0.7),
    ("我在用", Category::Context, 0.7),
    ("我正在用", Category::Context, 0.7),
    ("i like", Category::Like, 0.7),
    ("i love", Category::Like, 0.7),
    ("i hate", Category::Like, 0.7),
    ("i use", Category::Context, 0.7),
    ("every day i", Category::Habit, 0.7),
];

/// Correction markers: candidate supersedes the most recent active fact with
/// the highest content overlap.
const CORRECTION_MARKERS: &[&str] = &["说错了", "不对", "其实是", "我记错了", "not actually", "actually, i"];

/// Boundary markers → written straight to the boundaries table (highest
/// priority, plan §5.3), plus a `boundary`-category fact for traceability.
const BOUNDARY_MARKERS: &[&str] = &["别再", "不要再", "别在", "禁止", "别打断", "don't interrupt", "stop interrupting"];

fn overlap(a: &str, b: &str) -> usize {
    // Shared 2-gram count (bigrams): the weakest similarity that still
    // separates a correction from its target on short Chinese utterances,
    // while staying deterministic and allocation-light (P0 scope).
    let (a, b) = (normalize(a), normalize(b));
    a.chars()
        .collect::<Vec<_>>()
        .windows(2)
        .filter(|w| b.contains(&w.iter().collect::<String>()))
        .count()
}

/// The most overlapping active fact for a correction utterance, if any.
/// Threshold 2 shared bigrams = "same subject, plausibly about it".
pub fn correction_target<'a>(
    facts: &'a [ActiveFact],
    content: &str,
) -> Option<&'a ActiveFact> {
    facts
        .iter()
        .map(|f| (overlap(content, &f.content), f))
        .filter(|(hits, _)| *hits >= 2)
        .max_by_key(|(hits, _)| *hits)
        .map(|(_, f)| f)
}

pub struct RuleExtractor;

impl FactExtractor for RuleExtractor {
    fn extract(&self, digest: &SessionDigest) -> Vec<CandidateFact> {
        let mut out = Vec::new();
        for ev in digest.user_utterances {
            let lower = ev.content.to_lowercase();

            // 1) Explicit boundary — highest priority, wins over other rules.
            if BOUNDARY_MARKERS.iter().any(|m| lower.contains(m)) {
                out.push(CandidateFact {
                    content: ev.content.clone(),
                    category: Category::Boundary,
                    confidence: 0.95,
                    source_event_id: Some(ev.id),
                    is_correction: false,
                });
                continue;
            }

            // 2) Correction — target resolves at apply time (see
            //    CandidateFact::is_correction); apply_candidates skips it
            //    when nothing overlaps, so emitting unconditionally is safe.
            if CORRECTION_MARKERS.iter().any(|m| lower.contains(m)) {
                out.push(CandidateFact {
                    content: ev.content.clone(),
                    category: Category::Context,
                    confidence: 0.6,
                    source_event_id: Some(ev.id),
                    is_correction: true,
                });
                continue;
            }

            // 3) Direct statement.
            if let Some((_, category, confidence)) = DIRECT_PATTERNS
                .iter()
                .find(|(prefix, _, _)| lower.starts_with(prefix))
            {
                out.push(CandidateFact {
                    content: ev.content.clone(),
                    category: *category,
                    confidence: *confidence,
                    source_event_id: Some(ev.id),
                    is_correction: false,
                });
            }
            // 4) Everything else: no candidate. 宁漏勿误 — model layer (M5)
            //    widens this without touching the pipeline.
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(id: i64, content: &str) -> EventRow {
        EventRow {
            id,
            ts: "2026-11-18T00:00:00Z".into(),
            kind: "chat".into(),
            role: Some("user".into()),
            session_id: None,
            content: content.into(),
        }
    }

    #[test]
    fn extracts_direct_statements_with_category() {
        let evs = vec![
            event(1, "我喜欢雨天的声音"),
            event(2, "我每天早上十点才开工"),
            event(3, "我在用 Rust 写桌面项目"),
        ];
        let ex = RuleExtractor.extract(&SessionDigest {
            user_utterances: &evs,
            active_facts: &[],
        });
        assert_eq!(ex.len(), 3);
        assert_eq!(ex[0].category, Category::Like);
        assert_eq!(ex[1].category, Category::Habit);
        assert_eq!(ex[2].category, Category::Context);
        assert_eq!(ex[0].source_event_id, Some(1));
    }

    #[test]
    fn boundaries_win_and_skip_confidence_gates() {
        let evs = vec![event(1, "我喜欢雨天"), event(2, "别在我专注的时候弹窗")];
        let ex = RuleExtractor.extract(&SessionDigest {
            user_utterances: &evs,
            active_facts: &[],
        });
        assert_eq!(ex[1].category, Category::Boundary);
        assert_eq!(ex[1].confidence, 0.95);
    }

    #[test]
    fn correction_points_at_overlapping_fact() {
        let facts = vec![ActiveFact {
            id: 7,
            ts: "2026-11-17T00:00:00Z".into(),
            content: "用户喜欢下雨天".into(),
            confidence: 0.7,
            importance: 3,
            source_event_id: None,
        }];
        let evs = vec![event(1, "说错了，其实我不喜欢下雨天")];
        let ex = RuleExtractor.extract(&SessionDigest {
            user_utterances: &evs,
            active_facts: &facts,
        });
        assert_eq!(ex.len(), 1);
        assert!(ex[0].is_correction);
        // Target resolves at apply time; here we only verify the resolver.
        assert_eq!(correction_target(&facts, &evs[0].content).map(|f| f.id), Some(7));
    }

    #[test]
    fn correction_without_target_resolves_to_none() {
        let evs = vec![event(1, "说错了，其实是下周三截止")];
        let ex = RuleExtractor.extract(&SessionDigest {
            user_utterances: &evs,
            active_facts: &[],
        });
        assert_eq!(ex.len(), 1);
        assert!(correction_target(&[], &evs[0].content).is_none());
    }

    #[test]
    fn silence_beats_false_positives() {
        let evs = vec![event(1, "今天天气不错，我们出去走走吧"), event(2, "随便")];
        let ex = RuleExtractor.extract(&SessionDigest {
            user_utterances: &evs,
            active_facts: &[],
        });
        assert!(ex.is_empty());
    }

    #[test]
    fn normalization_strips_punct_and_case() {
        assert_eq!(normalize("我喜欢，雨天！"), normalize("我喜欢雨天"));
        assert_eq!(normalize("I LIKE Rust"), normalize("i like rust"));
    }
}

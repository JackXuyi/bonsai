//! Nightly consolidation job v2 (plan-memory.md §5 — report §4.3).
//!
//! Pipeline per run, single transaction, idempotent by construction:
//!   ② extract    window's unconsumed events → candidate facts (extractor)
//!   ③ merge      normalized duplicates bump confidence (plan §5.1)
//!   ④ contradict correction candidates stamp `invalid_at` on the old fact;
//!                targets resolve at APPLY time so same-run facts count
//!   ⑤ promote    confidence ≥ 0.8 → preference (idempotent via dedup guard);
//!                stale low-confidence facts are invalidated (defensive sweep)
//!   finish       mark window events consumed; write the audit row
//!
//! Stage ① (session summaries) is deliberately absent in P0 — see the plan's
//! open-question recommendation.
//!
//! Double idempotency (plan §5.4): events carry `consumed` and the audit row
//! records the window, so a retried run can never double-extract. On any
//! error the transaction rolls back: no loss, no duplication.

use rusqlite::Connection;
use serde::Serialize;

use super::db;
use super::extract::{CandidateFact, Category, FactExtractor, RuleExtractor, SessionDigest};

/// Facts at/above this confidence are promoted into the preferences layer.
const PROMOTION_THRESHOLD: f64 = 0.8;
/// Merge bump applied when a candidate repeats an existing active fact.
const REPEAT_BUMP: f64 = 0.1;
/// Deletion sweep: confidence below this AND older than 30 days → invalidate.
const DELETE_MAX_CONFIDENCE: f64 = 0.5;
const DELETE_AGE_DAYS: i64 = 30;

#[derive(Debug, Serialize)]
pub struct Report {
    pub trigger: String,
    pub window_start: String,
    pub window_end: String,
    pub events_in_window: usize,
    pub extracted: usize,
    pub merged: usize,
    pub contradicted: usize,
    pub promoted: usize,
    pub deleted: usize,
    pub consumed: usize,
}

pub struct Options<'a> {
    pub trigger: &'a str, // 'nightly' | 'catchup' | 'manual'
    pub window_start: &'a str,
    pub window_end: &'a str,
}

/// Run one consolidation pass.
pub fn run(
    conn: &mut Connection,
    extractor: &dyn FactExtractor,
    opts: &Options,
) -> rusqlite::Result<Report> {
    let tx = conn.transaction()?;
    let report = run_in_tx(&tx, extractor, opts)?;
    tx.commit()?;
    Ok(report) // on Err above, tx drops → rollback: events stay unconsumed
}

fn run_in_tx(
    conn: &Connection,
    extractor: &dyn FactExtractor,
    opts: &Options,
) -> rusqlite::Result<Report> {
    // Window events (unconsumed only — the second idempotency leg).
    let events = db::unconsumed_events(conn, opts.window_start, opts.window_end)?;
    let user_events: Vec<db::EventRow> = events
        .iter()
        .filter(|e| e.role.as_deref() != Some("assistant"))
        .cloned()
        .collect();

    let mut report = Report {
        trigger: opts.trigger.to_string(),
        window_start: opts.window_start.to_string(),
        window_end: opts.window_end.to_string(),
        events_in_window: events.len(),
        extracted: 0,
        merged: 0,
        contradicted: 0,
        promoted: 0,
        deleted: 0,
        consumed: 0,
    };

    if !events.is_empty() {
        let active_facts = db::active_facts(conn)?;
        let candidates = extractor.extract(&SessionDigest {
            user_utterances: &user_events,
            active_facts: &active_facts,
        });
        report.extracted = candidates.len();
        apply_candidates(conn, &candidates, &mut report)?;
    }

    // Deletion sweep runs every pass, window or not (defensive hygiene).
    report.deleted = delete_stale(conn)?;

    // Finish: mark consumed (only within the window — never touches events
    // outside it) and write the audit row.
    report.consumed = db::mark_events_consumed(conn, opts.window_start, opts.window_end)?;
    write_audit(conn, &report)?;
    Ok(report)
}

/// Stages ③④⑤ applied to the candidate list, in event order.
fn apply_candidates(
    conn: &Connection,
    candidates: &[CandidateFact],
    report: &mut Report,
) -> rusqlite::Result<()> {
    // Local snapshot, refreshed as we insert — later candidates in the same
    // run can bump or correct what earlier ones created.
    let mut facts = db::active_facts(conn)?;

    for cand in candidates {
        // Stage ④: corrections resolve their target against the CURRENT
        // snapshot (extraction time only had the pre-run state).
        if cand.is_correction {
            if let Some(old) = super::extract::correction_target(&facts, &cand.content) {
                let old_id = old.id;
                report.contradicted += db::invalidate_fact(conn, old_id)?;
                facts.retain(|f| f.id != old_id);
                let id =
                    db::add_fact_confident(conn, &cand.content, cand.source_event_id, cand.confidence)?;
                facts.push(new_fact(id, cand));
            }
            continue; // correction with no target: 宁漏勿误, skip
        }

        // Stage ③: merge. Normalized exact match bumps confidence instead of
        // inserting a duplicate.
        let norm = super::extract::normalize(&cand.content);
        let fact_id = match facts.iter_mut().find(|f| super::extract::normalize(&f.content) == norm) {
            Some(existing) => {
                db::bump_fact(conn, existing.id, REPEAT_BUMP)?;
                report.merged += 1;
                existing.confidence = ((existing.confidence + REPEAT_BUMP).min(0.95) * 100.0).round() / 100.0;
                existing.id
            }
            None => {
                let id =
                    db::add_fact_confident(conn, &cand.content, cand.source_event_id, cand.confidence)?;
                facts.push(new_fact(id, cand));
                id
            }
        };

        // Boundaries: straight to the boundaries table (plan §5.3), traced to
        // the fact for explainability; normalized dedup keeps the table clean.
        if cand.category == Category::Boundary {
            if !db::boundary_exists(conn, &cand.content)? {
                db::add_boundary(conn, &cand.content, Some(fact_id))?;
            }
            continue;
        }

        // Stage ⑤: promotion gate.
        let conf = facts
            .iter()
            .find(|f| f.id == fact_id)
            .map(|f| f.confidence)
            .unwrap_or(cand.confidence);
        if conf >= PROMOTION_THRESHOLD
            && db::promote_fact(conn, cand.category.as_str(), &cand.content, conf)?
        {
            report.promoted += 1;
        }
    }
    Ok(())
}

fn new_fact(id: i64, cand: &CandidateFact) -> db::ActiveFact {
    db::ActiveFact {
        id,
        ts: super::clock::now_iso(),
        content: cand.content.clone(),
        confidence: cand.confidence,
        importance: 3,
        source_event_id: cand.source_event_id,
    }
}

/// Stage ⑤ deletion sweep: stale low-confidence facts are invalidated, never
/// hard-deleted (system deletes stay soft — plan §3 invariant 2).
fn delete_stale(conn: &Connection) -> rusqlite::Result<usize> {
    let cutoff = chrono::DateTime::parse_from_rfc3339(&super::clock::now_iso())
        .map(|t| (t - chrono::Duration::days(DELETE_AGE_DAYS)).to_rfc3339())
        .unwrap_or_default();
    let ids = db::stale_low_confidence_facts(conn, &cutoff, DELETE_MAX_CONFIDENCE)?;
    let mut n = 0;
    for id in ids {
        n += db::invalidate_fact(conn, id)?;
    }
    Ok(n)
}

fn write_audit(conn: &Connection, report: &Report) -> rusqlite::Result<()> {
    let now = super::clock::now_iso();
    conn.execute(
        "INSERT INTO consolidate_runs
             (started_at, finished_at, trigger, window_start, window_end,
              extracted, merged, contradicted, promoted, deleted, consumed, status)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 'done')",
        rusqlite::params![
            now,
            now, // started == finished for P0; a long-running LocalExtractor run may differ later
            report.trigger,
            report.window_start,
            report.window_end,
            report.extracted as i64,
            report.merged as i64,
            report.contradicted as i64,
            report.promoted as i64,
            report.deleted as i64,
            report.consumed as i64,
        ],
    )?;
    Ok(())
}

/// Default P0 pipeline entry: rule extractor.
pub fn run_with_rules(conn: &mut Connection, opts: &Options) -> rusqlite::Result<Report> {
    run(conn, &RuleExtractor, opts)
}

/// Model-first entry (plan §5.2): use `LocalExtractor` when the local LLM
/// is reachable, fall back to rules otherwise. One cheap probe decides —
/// a dead server must never cost the pipeline its guarantees.
pub fn run_with_local_or_rules(conn: &mut Connection, opts: &Options) -> rusqlite::Result<Report> {
    let local = super::extract::LocalExtractor::new();
    if local.client.is_up() {
        log::info!("consolidate: model leg = LocalExtractor ({})", local.client.model());
        run(conn, &local, opts)
    } else {
        log::info!("consolidate: local LLM unreachable, falling back to RuleExtractor");
        run_with_rules(conn, opts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::clock;
    use rusqlite::Connection;

    fn mem() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(db::SCHEMA).unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        conn
    }

    const START: &str = "2026-11-17T00:00:00Z";
    const END: &str = "2026-11-19T00:00:00Z"; // events land 2026-11-18T12:00Z, strictly inside

    fn say(conn: &Connection, content: &str) -> i64 {
        db::add_event_meta(conn, "chat", content, Some("user"), Some("s1")).unwrap()
    }

    fn nightly() -> Options<'static> {
        Options { trigger: "nightly", window_start: START, window_end: END }
    }

    #[test]
    fn extracts_merges_contradicts_and_consumes() {
        let _guard = clock::pinned("2026-11-18T12:00:00Z");
        let mut conn = mem();

        say(&conn, "我喜欢雨天");
        say(&conn, "我每天散步半小时");
        say(&conn, "我喜欢雨天。"); // punctuation differs → merge
        say(&conn, "说错了，其实是我不喜欢雨天"); // correction targets the rainy fact

        let report = run_with_rules(&mut conn, &nightly()).unwrap();
        assert_eq!(report.extracted, 4);
        assert_eq!(report.merged, 1);
        assert_eq!(report.contradicted, 1);
        assert_eq!(report.consumed, 4);

        // Re-running the same window is a no-op (double idempotency).
        let again = run_with_rules(
            &mut conn,
            &Options { trigger: "catchup", window_start: START, window_end: END },
        )
        .unwrap();
        assert_eq!(again.extracted, 0);
        assert_eq!(again.consumed, 0);
        assert_eq!(again.events_in_window, 0);
    }

    #[test]
    fn promotion_after_second_mention_is_idempotent() {
        let _guard = clock::pinned("2026-11-18T12:00:00Z");
        let mut conn = mem();

        say(&conn, "我喜欢雨天");
        let r1 = run_with_rules(
            &mut conn,
            &Options { trigger: "manual", window_start: START, window_end: END },
        )
        .unwrap();
        assert_eq!(r1.promoted, 0); // 0.7 < 0.8 — not confident enough yet

        say(&conn, "我喜欢雨天");
        let r2 = run_with_rules(
            &mut conn,
            &Options { trigger: "manual", window_start: START, window_end: END },
        )
        .unwrap();
        assert_eq!(r2.merged, 1); // 0.7 + 0.1 = 0.8
        assert_eq!(r2.promoted, 1);

        let prefs: i64 = conn
            .query_row("SELECT COUNT(*) FROM preferences WHERE category = 'like'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(prefs, 1); // promote_fact dedup guard held
    }

    #[test]
    fn boundary_goes_straight_to_boundaries_table() {
        let _guard = clock::pinned("2026-11-18T12:00:00Z");
        let mut conn = mem();
        say(&conn, "别在我专注的时候弹窗");
        say(&conn, "别在我专注的时候弹窗"); // duplicate phrasing → dedup

        let report = run_with_rules(
            &mut conn,
            &Options { trigger: "manual", window_start: START, window_end: END },
        )
        .unwrap();
        assert_eq!(report.extracted, 2);
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM boundaries", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);
        // Boundary rows trace back to a fact (traceability invariant).
        let linked: i64 = conn
            .query_row("SELECT COUNT(*) FROM boundaries WHERE source_fact_id IS NOT NULL", [], |r| r.get(0))
            .unwrap();
        assert_eq!(linked, 1);
    }

    #[test]
    fn deletion_sweep_invalidates_stale_low_confidence() {
        let _guard = clock::pinned("2026-11-18T12:00:00Z");
        let mut conn = mem();
        let old_id = db::add_fact_confident(&conn, "stale guess", None, 0.4).unwrap();
        conn.execute("UPDATE facts SET ts = '2026-10-01T00:00:00Z' WHERE id = ?1", [old_id])
            .unwrap();

        let report = run_with_rules(&mut conn, &nightly()).unwrap(); // empty window, sweep still runs
        assert_eq!(report.deleted, 1);
        let active: i64 = conn
            .query_row("SELECT COUNT(*) FROM facts WHERE invalid_at IS NULL", [], |r| r.get(0))
            .unwrap();
        assert_eq!(active, 0);
    }

    #[test]
    fn error_rolls_back_and_keeps_events_unconsumed() {
        let _guard = clock::pinned("2026-11-18T12:00:00Z");
        let mut conn = mem();
        say(&conn, "我喜欢雨天");

        // Candidate references a non-existent event → FK violation mid-run.
        struct BadSource;
        impl FactExtractor for BadSource {
            fn extract(&self, _d: &SessionDigest) -> Vec<CandidateFact> {
                vec![CandidateFact {
                    content: "ghost fact".into(),
                    category: Category::Like,
                    confidence: 0.7,
                    source_event_id: Some(999_999),
                    is_correction: false,
                }]
            }
        }

        let result = run(&mut conn, &BadSource, &nightly());
        assert!(result.is_err());

        // Nothing committed: event still pending, no facts, no audit rows.
        let pending = db::unconsumed_events(&conn, START, END).unwrap();
        assert_eq!(pending.len(), 1);
        let facts: i64 = conn.query_row("SELECT COUNT(*) FROM facts", [], |r| r.get(0)).unwrap();
        assert_eq!(facts, 0);
        let audits: i64 =
            conn.query_row("SELECT COUNT(*) FROM consolidate_runs", [], |r| r.get(0)).unwrap();
        assert_eq!(audits, 0);
    }
}

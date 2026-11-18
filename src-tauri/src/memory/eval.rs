//! Memory evaluation — the three first-class metrics (plan-memory.md §8,
//! report §4.4). These run in CI on every change to extraction/retrieval:
//! a metrics regression is a build failure, not a note in a doc.
//!
//! Method: rule extractor + pinned clock (plan §8), deterministic corpus.
//!   1. Hit rate        — fact retrievable via a question that depends on it  > 85%
//!   2. Misremember rate— top hit must never be a superseded value           <  1%
//!   3. Decay correctness— expired memories never outrank fresh ones        = 100%

#[cfg(test)]
mod tests {
    use crate::memory::clock;
    use crate::memory::consolidate::{self, Options};
    use crate::memory::db::{self, SCHEMA};
    use rusqlite::Connection;

    const NOW: &str = "2026-12-01T00:00:00Z";
    // Window strictly contains the seeded events (ts == NOW, window ends a
    // day later — `unconsumed_events` selects ts < window_end).
    const WINDOW_END: &str = "2026-12-02T00:00:00Z";
    const WINDOW_START: &str = "2026-11-30T00:00:00Z";

    fn seeded_conn(n_topics: usize) -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        let _guard = clock::pinned(NOW);

        // Seed dialogue: 50 direct statements. The rule extractor lifts each
        // into a fact (confidence 0.7, importance 3).
        for i in 0..n_topics {
            db::add_event_meta(
                &conn,
                "chat",
                &format!("I like topic{i:02} theme"),
                Some("user"),
                Some("eval"),
            )
            .unwrap();
        }
        let mut conn = conn;
        consolidate::run_with_rules(
            &mut conn,
            &Options {
                trigger: "manual",
                window_start: WINDOW_START,
                window_end: WINDOW_END,
            },
        )
        .unwrap();
        conn
    }

    /// Metric 1 — 记忆命中率: for each stored fact, ask the question that
    /// depends on it ("what does the user like about topicNN?") and check the
    /// fact lands in the top 10.
    #[test]
    fn hit_rate_above_85_percent() {
        let _guard = clock::pinned(NOW);
        let conn = seeded_conn(50);

        let mut hits = 0;
        for i in 0..50 {
            let results = db::search(&conn, &format!("topic{i:02}"), 10, true).unwrap();
            let found = results
                .iter()
                .any(|h| h.kind == "fact" && !h.invalidated && h.content.contains(&format!("topic{i:02}")));
            if found {
                hits += 1;
            }
        }
        let rate = hits as f64 / 50.0;
        assert!(rate > 0.85, "hit rate {rate:.2} ({hits}/50) below target 0.85");
    }

    /// Metric 2 — 误记率: 5 corrections poison 5 old facts; for those topics
    /// the top hit must be an ACTIVE fact, never the superseded value.
    #[test]
    fn misremember_rate_below_1_percent() {
        let _guard = clock::pinned(NOW);
        let mut conn = seeded_conn(50);

        for i in 0..5 {
            db::add_event_meta(
                &conn,
                "chat",
                &format!("说错了，其实我不喜欢topic{i:02} theme"),
                Some("user"),
                Some("eval"),
            )
            .unwrap();
        }
        consolidate::run_with_rules(
            &mut conn,
            &Options {
                trigger: "manual",
                window_start: WINDOW_START,
                window_end: WINDOW_END,
            },
        )
        .unwrap();

        // Questions whose correct answer is the ACTIVE (corrected) state.
        let mut misremembered = 0;
        for i in 0..5 {
            let results = db::search(&conn, &format!("topic{i:02}"), 3, true).unwrap();
            let top_is_active_fact = results
                .first()
                .map(|h| h.kind == "fact" && !h.invalidated)
                .unwrap_or(false);
            if !top_is_active_fact {
                misremembered += 1;
            }
        }
        let rate = misremembered as f64 / 50.0; // over the whole 50-question set
        assert!(rate < 0.01, "misremember rate {rate:.3} ({misremembered}/50) at/above 1%");
    }

    /// Metric 3 — 衰减正确率: 30 sixty-day-old memories must never outrank a
    /// fresh memory for the same question.
    #[test]
    fn decayed_memories_never_outrank_fresh() {
        let _guard = clock::pinned(NOW);
        let conn = seeded_conn(0);
        let mut ok = 0;

        for i in 0..30 {
            let old = db::add_fact_confident(&conn, &format!("stale{i:02} old fact"), None, 0.7)
                .unwrap();
            conn.execute(
                "UPDATE facts SET ts = '2026-10-01T00:00:00Z' WHERE id = ?1",
                [old],
            )
            .unwrap();
            db::add_fact_confident(&conn, &format!("stale{i:02} new fact"), None, 0.7).unwrap();

            let results = db::search(&conn, &format!("stale{i:02}"), 5, true).unwrap();
            let first_is_fresh = results
                .first()
                .map(|h| h.content.contains("new"))
                .unwrap_or(false);
            if first_is_fresh {
                ok += 1;
            }
        }
        assert_eq!(ok, 30, "decay correctness {ok}/30 — expired memories outranked fresh ones");
    }
}

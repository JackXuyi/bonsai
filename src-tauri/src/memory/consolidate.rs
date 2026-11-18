//! Nightly consolidation job (report §4.3 — the underestimated key mechanism).
//!
//! Pipeline: compress the day's dialogue → extract facts → merge duplicates →
//! resolve contradictions (stamp invalid_at) → delete low-long-term-value items.
//!
//! Effects claimed by the research: memory omissions drop ~30%; N raw dialogues
//! compress to M structured facts (M << N), so next-day prompts get much shorter.
//!
//! Scaffold scope: exact-duplicate merging only. Extraction and contradiction
//! resolution need the model layer and are follow-up milestones.

use rusqlite::Connection;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct Report {
    pub merged_duplicate_facts: usize,
}

pub fn run(conn: &Connection) -> rusqlite::Result<Report> {
    let merged = merge_duplicate_facts(conn)?;
    Ok(Report {
        merged_duplicate_facts: merged,
    })
}

/// Among active (not invalidated) facts, keep the earliest copy of each exact
/// content match and remove the rest.
fn merge_duplicate_facts(conn: &Connection) -> rusqlite::Result<usize> {
    conn.execute(
        "DELETE FROM facts
         WHERE invalid_at IS NULL
           AND id NOT IN (
               SELECT MIN(id) FROM facts WHERE invalid_at IS NULL GROUP BY content
           )",
        [],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::db;

    fn mem() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(db::SCHEMA).unwrap();
        conn
    }

    #[test]
    fn merges_exact_duplicates() {
        let conn = mem();
        db::add_fact(&conn, "user loves rainy days", None).unwrap();
        db::add_fact(&conn, "user loves rainy days", None).unwrap();
        db::add_fact(&conn, "user hates group chats", None).unwrap();

        let report = run(&conn).unwrap();
        assert_eq!(report.merged_duplicate_facts, 1);

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM facts WHERE invalid_at IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 2);
    }
}

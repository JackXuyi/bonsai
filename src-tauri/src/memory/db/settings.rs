//! Settings KV + onboarding audit answers.

use rusqlite::{params, Connection};

use crate::memory::clock;

// ----------------------------------------------- settings + onboarding audit
// (docs/zh/plan-onboarding.md §3.1)

/// Read a settings key. Missing key → None (this is how "old user without
/// onboarding_done" is detected: they get the flow on next window open).
pub fn get_setting(conn: &Connection, key: &str) -> rusqlite::Result<Option<String>> {
    match conn.query_row("SELECT value FROM settings WHERE key = ?1", params![key], |r| r.get(0)) {
        Ok(v) => Ok(Some(v)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Upsert a settings key.
pub fn set_setting(conn: &Connection, key: &str, value: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO settings (key, value, updated_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
        params![key, value, clock::now_iso()],
    )?;
    Ok(())
}

/// Clear a settings key (the tray "redo onboarding" path: keep the answers,
/// drop only the done flag).
pub fn remove_setting(conn: &Connection, key: &str) -> rusqlite::Result<usize> {
    conn.execute("DELETE FROM settings WHERE key = ?1", params![key])
}

/// One onboarding answer, audit form (skipped answers included).
pub fn record_onboarding_answer(
    conn: &Connection,
    question: &str,
    answer: &str,
    skipped: bool,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO onboarding_answers (question, answer, skipped, ts) VALUES (?1, ?2, ?3, ?4)",
        params![question, answer, skipped as i64, clock::now_iso()],
    )?;
    Ok(())
}

/// All recorded answers, oldest first.
pub fn onboarding_answers(conn: &Connection) -> rusqlite::Result<Vec<(String, String, bool)>> {
    let mut stmt = conn.prepare(
        "SELECT question, answer, skipped FROM onboarding_answers ORDER BY id ASC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)? != 0,
        ))
    })?;
    rows.collect()
}


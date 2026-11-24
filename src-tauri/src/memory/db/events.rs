//! Events / facts / boundaries / preferences writes + forget + retrieval reads.

use rusqlite::{params, Connection};

use crate::memory::clock;

// ---------------------------------------------------------------- row types

#[derive(Debug, Clone)]
pub struct EventRow {
    pub id: i64,
    pub ts: String,
    pub kind: String,
    pub role: Option<String>,
    pub session_id: Option<String>,
    pub content: String,
}

#[derive(Debug, Clone)]
pub struct ActiveFact {
    pub id: i64,
    pub ts: String,
    pub content: String,
    pub confidence: f64,
    pub importance: i64,
    pub source_event_id: Option<i64>,
}

// ---------------------------------------------------------------- writes

/// Append a raw event (scaffold-compatible signature used by face.rs).
pub fn add_event(conn: &Connection, kind: &str, content: &str) -> rusqlite::Result<i64> {
    add_event_meta(conn, kind, content, None, None)
}

/// Append a raw event with dialogue metadata (role / session).
///
/// v15：对话事件是**唯一**能决定「会话存在与最近活跃」的写入点，所以
/// 会话索引在这里顺手维护（`session_touch`）——调用方散落在危机拦截、
/// 日历路由、云端循环、总结、本地直答五处，各自记得 touch 一定会漏。
/// `proactive` 例外：主动搭话不进会话列表（plan-context D6）。
pub fn add_event_meta(
    conn: &Connection,
    kind: &str,
    content: &str,
    role: Option<&str>,
    session_id: Option<&str>,
) -> rusqlite::Result<i64> {
    conn.execute(
        "INSERT INTO events (ts, kind, content, role, session_id) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![clock::now_iso(), kind, content, role, session_id],
    )?;
    let id = conn.last_insert_rowid();
    if kind == "dialogue" {
        if let Some(sid) = session_id {
            if sid != super::PROACTIVE_SESSION_ID {
                let title_from = if role == Some("user") { Some(content) } else { None };
                super::session_touch(conn, sid, title_from)?;
            }
        }
    }
    Ok(id)
}

pub fn add_fact(conn: &Connection, content: &str, source_event_id: Option<i64>) -> rusqlite::Result<i64> {
    add_fact_confident(conn, content, source_event_id, 0.6)
}

pub fn add_fact_confident(
    conn: &Connection,
    content: &str,
    source_event_id: Option<i64>,
    confidence: f64,
) -> rusqlite::Result<i64> {
    conn.execute(
        "INSERT INTO facts (ts, content, source_event_id, confidence) VALUES (?1, ?2, ?3, ?4)",
        params![clock::now_iso(), content, source_event_id, confidence],
    )?;
    let id = conn.last_insert_rowid();
    if let Some(eid) = source_event_id {
        crate::memory::evidence::link_fact(conn, id, eid)?;
    }
    Ok(id)
}

/// Quantize confidence to two decimals — `0.7 + 0.1` must land exactly on
/// the 0.8 promotion gate, not on f64's 0.7999999999999999.
pub(super) fn quantize_conf(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

/// Repeated mention: confidence rises by `delta`, capped at 0.95.
pub fn bump_fact(conn: &Connection, fact_id: i64, delta: f64) -> rusqlite::Result<()> {
    let current: f64 = conn.query_row(
        "SELECT confidence FROM facts WHERE id = ?1",
        params![fact_id],
        |r| r.get(0),
    )?;
    let next = quantize_conf((current + delta).min(0.95));
    conn.execute(
        "UPDATE facts SET confidence = ?1 WHERE id = ?2",
        params![next, fact_id],
    )?;
    Ok(())
}

/// Mark a fact as superseded (soft delete / contradiction resolution).
/// The old value is never rewritten — `invalid_at` preserves the
/// "what we used to believe" history (plan §3 invariant 2).
pub fn invalidate_fact(conn: &Connection, fact_id: i64) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE facts SET invalid_at = ?1 WHERE id = ?2 AND invalid_at IS NULL",
        params![clock::now_iso(), fact_id],
    )
}

/// Boundary write: highest priority, bypasses confidence gates (plan §5.3).
pub fn add_boundary(
    conn: &Connection,
    content: &str,
    source_fact_id: Option<i64>,
) -> rusqlite::Result<i64> {
    conn.execute(
        "INSERT INTO boundaries (ts, content, source_fact_id) VALUES (?1, ?2, ?3)",
        params![clock::now_iso(), content, source_fact_id],
    )?;
    let id = conn.last_insert_rowid();
    if let Some(fid) = source_fact_id {
        let event_id: Option<i64> = conn
            .query_row(
                "SELECT source_event_id FROM facts WHERE id = ?1",
                params![fid],
                |r| r.get(0),
            )
            .unwrap_or(None);
        if let Some(eid) = event_id {
            crate::memory::evidence::link_boundary(conn, id, eid)?;
        }
    }
    Ok(id)
}

/// Normalized-equality guard so repeated boundary phrasings don't pile up.
pub fn boundary_exists(conn: &Connection, content: &str) -> rusqlite::Result<bool> {
    let norm = crate::memory::extract::normalize(content);
    let mut stmt = conn.prepare("SELECT content FROM boundaries")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    for row in rows {
        if crate::memory::extract::normalize(&row?) == norm {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Preference write（可无源——历史/测试孤儿；升格请走 `promote_fact`）。
pub fn add_preference(
    conn: &Connection,
    category: &str,
    content: &str,
    confidence: f64,
) -> rusqlite::Result<i64> {
    conn.execute(
        "INSERT INTO preferences (ts, category, content, confidence) VALUES (?1, ?2, ?3, ?4)",
        params![clock::now_iso(), category, content, confidence],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Promote a fact only if no active preference holds the same content.
/// `source_event_ids` 至少 1 个（D3：禁止无源升格）；已存在同 content 时
/// 合并补写 evidence，返回 false（未新建行）。
pub fn promote_fact(
    conn: &Connection,
    category: &str,
    content: &str,
    confidence: f64,
    source_event_ids: &[i64],
) -> rusqlite::Result<bool> {
    if source_event_ids.is_empty() {
        return Err(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(1),
            Some("promote_fact requires at least one source_event_id".into()),
        ));
    }
    let existing_id: Option<i64> = conn
        .query_row(
            "SELECT id FROM preferences WHERE content = ?1 AND invalid_at IS NULL LIMIT 1",
            params![content],
            |r| r.get(0),
        )
        .ok();
    let pref_id = match existing_id {
        Some(id) => id,
        None => add_preference(conn, category, content, confidence)?,
    };
    for eid in source_event_ids {
        crate::memory::evidence::link_preference(conn, pref_id, *eid)?;
    }
    Ok(existing_id.is_none())
}

// --------------------------------------------------- forget (compliance Art. 16)
// 权威实现：`memory::forget`（plan-reliability-phase1 R1）。

pub fn forget_event(conn: &Connection, event_id: i64) -> rusqlite::Result<usize> {
    crate::memory::forget::forget_event(conn, event_id)
}

/// 返回删除的 events 行数（沿用旧语义）。
pub fn forget_session(conn: &Connection, session_id: &str) -> rusqlite::Result<usize> {
    crate::memory::forget::forget_session(conn, session_id)
}

pub fn forget_all(conn: &Connection) -> rusqlite::Result<()> {
    crate::memory::forget::forget_all(conn)
}

/// Reset ONLY the relationship line (tool-page button, plan §3.8): the user
/// may want to "start over" socially without losing memory or the plant.
pub fn forget_relationship(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "DELETE FROM relationship_log; DELETE FROM relationship_progress; DELETE FROM relationship;",
    )?;
    Ok(())
}

// ------------------------------------------------------- reads for consolidate

/// Unconsumed events inside `[window_start, window_end)`, oldest first.
pub fn unconsumed_events(
    conn: &Connection,
    window_start: &str,
    window_end: &str,
) -> rusqlite::Result<Vec<EventRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, ts, kind, role, session_id, content FROM events
         WHERE consumed = 0 AND ts >= ?1 AND ts < ?2 ORDER BY id ASC",
    )?;
    let rows = stmt.query_map(params![window_start, window_end], |row| {
        Ok(EventRow {
            id: row.get(0)?,
            ts: row.get(1)?,
            kind: row.get(2)?,
            role: row.get(3)?,
            session_id: row.get(4)?,
            content: row.get(5)?,
        })
    })?;
    rows.collect()
}

pub fn mark_events_consumed(
    conn: &Connection,
    window_start: &str,
    window_end: &str,
) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE events SET consumed = 1 WHERE consumed = 0 AND ts >= ?1 AND ts < ?2",
        params![window_start, window_end],
    )
}

pub fn active_facts(conn: &Connection) -> rusqlite::Result<Vec<ActiveFact>> {
    let mut stmt = conn.prepare(
        "SELECT id, ts, content, confidence, importance, source_event_id FROM facts
         WHERE invalid_at IS NULL ORDER BY id ASC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(ActiveFact {
            id: row.get(0)?,
            ts: row.get(1)?,
            content: row.get(2)?,
            confidence: row.get(3)?,
            importance: row.get(4)?,
            source_event_id: row.get(5)?,
        })
    })?;
    rows.collect()
}

/// Active user boundaries (prompt-injection source, plan-persona §3.2
/// 双源对齐: card boundaries + user-collected boundaries merged at render).
pub fn boundaries(conn: &Connection) -> rusqlite::Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT content FROM boundaries ORDER BY id ASC")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    rows.collect()
}

/// Facts whose ts is older than `iso_cutoff` and confidence below `max_conf`.
/// Used by stage ⑤ deletion (defensive: rule-extracted facts stay ≥ 0.5,
/// so this currently only sweeps legacy/manual low-confidence entries).
pub fn stale_low_confidence_facts(
    conn: &Connection,
    iso_cutoff: &str,
    max_conf: f64,
) -> rusqlite::Result<Vec<i64>> {
    let mut stmt = conn.prepare(
        "SELECT id FROM facts
         WHERE invalid_at IS NULL AND confidence < ?1 AND ts < ?2 ORDER BY id ASC",
    )?;
    let rows = stmt.query_map(params![max_conf, iso_cutoff], |row| row.get(0))?;
    rows.collect()
}

// ---------------------------------------------------------- time-window reads

/// All events in an ISO-time window `[start, end)`, oldest first.
/// Feeds the daily report and the proactive-candidate value heuristic.
pub fn events_in_window(
    conn: &Connection,
    start_iso: &str,
    end_iso: &str,
) -> rusqlite::Result<Vec<EventRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, ts, kind, role, session_id, content FROM events
         WHERE ts >= ?1 AND ts < ?2 ORDER BY id ASC",
    )?;
    let rows = stmt.query_map(params![start_iso, end_iso], |row| {
        Ok(EventRow {
            id: row.get(0)?,
            ts: row.get(1)?,
            kind: row.get(2)?,
            role: row.get(3)?,
            session_id: row.get(4)?,
            content: row.get(5)?,
        })
    })?;
    rows.collect()
}

/// Facts created in the window (regardless of validity — the report shows
/// what was *learned* that day, corrections included).
pub fn facts_in_window(
    conn: &Connection,
    start_iso: &str,
    end_iso: &str,
) -> rusqlite::Result<Vec<ActiveFact>> {
    let mut stmt = conn.prepare(
        "SELECT id, ts, content, confidence, importance, source_event_id FROM facts
         WHERE ts >= ?1 AND ts < ?2 ORDER BY id ASC",
    )?;
    let rows = stmt.query_map(params![start_iso, end_iso], |row| {
        Ok(ActiveFact {
            id: row.get(0)?,
            ts: row.get(1)?,
            content: row.get(2)?,
            confidence: row.get(3)?,
            importance: row.get(4)?,
            source_event_id: row.get(5)?,
        })
    })?;
    rows.collect()
}

/// Window rollup for the summary engine (plan-summary-skill). Three signals
/// a summary may *talk about* without inventing anything: which tools were
/// used, how the guesses went, and how the relationship line moved. Counts
/// only — no content leaves this struct, so a summary prompt can never leak
/// calendar titles or tool arguments.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WindowStats {
    /// `tool:action` → count, top 5 by frequency.
    pub tools: Vec<(String, i64)>,
    pub predictions_asked: i64,
    pub predictions_hit: i64,
    pub predictions_miss: i64,
    pub relationship_score: i64,
    pub relationship_days: i64,
}

/// `start_iso`/`end_iso` bound the event-time tables (tool_calls); the two
/// day strings bound the day-keyed tables (predictions, relationship_progress).
/// Both windows come from the same local span, so they always agree.
pub fn window_stats(
    conn: &Connection,
    start_iso: &str,
    end_iso: &str,
    start_day: &str,
    end_day: &str,
) -> rusqlite::Result<WindowStats> {
    let mut out = WindowStats::default();

    let mut stmt = conn.prepare(
        "SELECT tool || ':' || action AS name, COUNT(*) FROM tool_calls
         WHERE ts >= ?1 AND ts < ?2 GROUP BY tool, action ORDER BY 2 DESC, name ASC LIMIT 5",
    )?;
    let rows = stmt.query_map(params![start_iso, end_iso], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;
    out.tools = rows.collect::<rusqlite::Result<Vec<_>>>()?;

    let (asked, hit, miss): (i64, i64, i64) = conn.query_row(
        "SELECT
           COALESCE(SUM(CASE WHEN asked_at IS NOT NULL THEN 1 ELSE 0 END), 0),
           COALESCE(SUM(CASE WHEN feedback = 'hit' THEN 1 ELSE 0 END), 0),
           COALESCE(SUM(CASE WHEN feedback = 'miss' THEN 1 ELSE 0 END), 0)
         FROM predictions WHERE date >= ?1 AND date <= ?2",
        params![start_day, end_day],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    out.predictions_asked = asked;
    out.predictions_hit = hit;
    out.predictions_miss = miss;

    let (score, days): (i64, i64) = conn.query_row(
        "SELECT COALESCE(SUM(score), 0), COUNT(*) FROM relationship_progress
         WHERE date >= ?1 AND date <= ?2",
        params![start_day, end_day],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    out.relationship_score = score;
    out.relationship_days = days;

    Ok(out)
}

// ------------------------------------------------- retrieval (M4: decayed hybrid)
/// Parse an RFC3339 ts; unparsable strings are treated as "fresh" (age 0).
fn age_days(ts: &str) -> f64 {
    let now = chrono::DateTime::parse_from_rfc3339(&clock::now_iso()).ok();
    let then = chrono::DateTime::parse_from_rfc3339(ts).ok();
    match (now, then) {
        (Some(n), Some(t)) => ((n - t).num_seconds() as f64 / 86_400.0).max(0.0),
        _ => 0.0,
    }
}

fn decay(age: f64) -> f64 {
    (-age / 45.0).exp() // plan §7 pins τ = 45 days
}

/// 事实行的统一打分（plan §7）：重要性 × 置信度 × 时间衰减，失效 ×0.1。
/// `db::search` 与 `salient_facts` 共用——两处各写一遍必然漂移。
fn fact_score(
    importance: i64,
    confidence: f64,
    ts: &str,
    invalidated: bool,
    with_decayed: bool,
) -> f64 {
    (importance as f64 / 3.0)
        * (0.5 + 0.5 * confidence)
        * if with_decayed { decay(age_days(ts)) } else { 1.0 }
        * if invalidated { 0.1 } else { 1.0 }
}

/// `IN (?,?,?)` 的占位串（ids 非空时才能调用）。
fn placeholders(n: usize) -> String {
    std::iter::repeat("?").take(n).collect::<Vec<_>>().join(",")
}

/// Keyword leg of hybrid retrieval across all three layers, with time-decay
/// scoring (plan §7). Boundaries and preferences are exempt from decay and
/// rank first; facts get importance × confidence × decay; events decay too.
/// `with_decayed = false` turns decay off ("翻旧账" mode for growth stories).
///
/// S2（2026-11-23）：候选集改由 `index` 出（FTS5 + 自建 2-gram），不再对
/// 四张表各扫一遍 `LIKE '%q%'`。**打分公式一个字没改**——换的是「谁能进
/// 候选」，不是「候选怎么排」，所以既有精度/召回指标保持可比。顺带补上
/// 旧实现缺的能力：旧的是「子串命中即全部同分，再按时间截断」，同一批
/// 命中里谁更相关完全看不出来；现在是 bm25 先筛候选，再由时间/重要性排序。
pub fn search(
    conn: &Connection,
    query: &str,
    limit: usize,
    with_decayed: bool,
) -> rusqlite::Result<Vec<crate::memory::MemoryHit>> {
    // 候选池放大 8 倍再做重排：bm25 只看词频，最终排序要看 decay/importance，
    // 池子太小会让「刚说过但词频低」的行拿不到出场机会。
    let pool = (limit.saturating_mul(8)).clamp(16, 256);
    crate::memory::index::sync(conn)?;
    let cands = crate::memory::index::search(conn, query, pool)?;
    if cands.is_empty() {
        return Ok(Vec::new());
    }

    let ids_of = |kind: &str| -> Vec<i64> {
        cands
            .iter()
            .filter(|(k, _)| k == kind)
            .map(|(_, id)| *id)
            .collect()
    };
    let mut scored: Vec<(f64, crate::memory::MemoryHit)> = Vec::new();

    // Boundaries: constant top priority, never decayed.
    let ids = ids_of("boundary");
    if !ids.is_empty() {
        let sql = format!(
            "SELECT id, ts, content FROM boundaries WHERE id IN ({})",
            placeholders(ids.len())
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(ids.iter()), |row| {
            Ok(crate::memory::MemoryHit {
                kind: "boundary".into(),
                id: row.get(0)?,
                ts: row.get(1)?,
                content: row.get(2)?,
                invalidated: false,
            })
        })?;
        for row in rows {
            scored.push((1.0, row?));
        }
    }

    // Preferences: constant, just below boundaries; invalidated ones sink.
    let ids = ids_of("preference");
    if !ids.is_empty() {
        let sql = format!(
            "SELECT id, ts, content, invalid_at FROM preferences WHERE id IN ({})",
            placeholders(ids.len())
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(ids.iter()), |row| {
            let hit = crate::memory::MemoryHit {
                kind: "preference".into(),
                id: row.get(0)?,
                ts: row.get(1)?,
                content: row.get(2)?,
                invalidated: row.get::<_, Option<String>>(3)?.is_some(),
            };
            // R2：默认硬排除失效记忆（不再 ×0.1 混入当前召回）。
            if hit.invalidated {
                return Ok(None);
            }
            Ok(Some((0.9, hit)))
        })?;
        for row in rows {
            if let Some(pair) = row? {
                scored.push(pair);
            }
        }
    }

    // Facts: importance/3 × (0.5 + 0.5·confidence) × decay.
    // R2：invalidated 硬排除（plan-reliability-phase1）。
    let ids = ids_of("fact");
    if !ids.is_empty() {
        let sql = format!(
            "SELECT id, ts, content, invalid_at, confidence, importance FROM facts
              WHERE id IN ({})",
            placeholders(ids.len())
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(ids.iter()), |row| {
            let invalidated = row.get::<_, Option<String>>(3)?.is_some();
            if invalidated {
                return Ok(None);
            }
            let hit = crate::memory::MemoryHit {
                kind: "fact".into(),
                id: row.get(0)?,
                ts: row.get(1)?,
                content: row.get(2)?,
                invalidated: false,
            };
            let confidence: f64 = row.get(4)?;
            let importance: i64 = row.get(5)?;
            let score = fact_score(importance, confidence, &hit.ts, false, with_decayed);
            Ok(Some((score, hit)))
        })?;
        for row in rows {
            if let Some(pair) = row? {
                scored.push(pair);
            }
        }
    }

    // Events: plain decay.
    let ids = ids_of("event");
    if !ids.is_empty() {
        let sql = format!(
            "SELECT id, ts, content FROM events WHERE id IN ({})",
            placeholders(ids.len())
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(ids.iter()), |row| {
            Ok(crate::memory::MemoryHit {
                kind: "event".into(),
                id: row.get(0)?,
                ts: row.get(1)?,
                content: row.get(2)?,
                invalidated: false,
            })
        })?;
        for row in rows {
            let hit = row?;
            let score = 0.6 * if with_decayed { decay(age_days(&hit.ts)) } else { 1.0 };
            scored.push((score, hit));
        }
    }

    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    Ok(scored.into_iter().map(|(_, hit)| hit).take(limit).collect())
}

/// 「最该被想起来的几条事实」——无查询词的召回兜底（S2）。
///
/// 「还记得吗」这类问法没有任何内容词，关键词腿为空，旧行为是把窗口内
/// 最近 60 条对话整段倒出来（S1 基线里这一行是 0%：倒出来的是噪声，
/// 目标事实在里面也被挤掉了）。人这时候期待的是「我最在意的那几件事」，
/// 所以按 importance × confidence × decay 取前 N 条有效事实。
pub fn salient_facts(conn: &Connection, limit: usize) -> rusqlite::Result<Vec<crate::memory::MemoryHit>> {
    let mut stmt = conn.prepare(
        "SELECT id, ts, content, confidence, importance FROM facts
          WHERE invalid_at IS NULL",
    )?;
    let rows = stmt.query_map([], |row| {
        let hit = crate::memory::MemoryHit {
            kind: "fact".into(),
            id: row.get(0)?,
            ts: row.get(1)?,
            content: row.get(2)?,
            invalidated: false,
        };
        let score = fact_score(row.get(4)?, row.get(3)?, &hit.ts, false, true);
        Ok((score, hit))
    })?;
    let mut scored: Vec<(f64, crate::memory::MemoryHit)> = rows.collect::<rusqlite::Result<_>>()?;
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    Ok(scored.into_iter().map(|(_, hit)| hit).take(limit).collect())
}

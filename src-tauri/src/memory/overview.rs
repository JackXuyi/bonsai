//! 记忆总览（spec-m3 §1）：只读快照，给记忆页用。
//!
//! 三条约束：
//! 1. **纯 SELECT**——这个命令不写库，也不碰 settings（测试用快照对比守着）。
//! 2. **不做时区转换**——ts 原样返回 RFC3339，本地化交前端（predict.rs 的教训：
//!    时间比较只在 Rust 侧，展示转换在前端）。
//! 3. **派生事件如实展示**——`face_state` / `disagreement*` / `relationship_*`
//!    不进 ACTIVITY_KINDS，但它们确实在库里；`derived` 标记让前端灰显，
//!    而不是把它们藏起来（记忆可看 = 信任）。

use rusqlite::Connection;
use serde::Serialize;

/// 默认/上限（spec-m3 §1）。上限存在的意义：预览面板宽度固定，再多也看不完。
pub const DEFAULT_LIMIT: i64 = 20;
pub const MAX_LIMIT: i64 = 100;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MemoryOverview {
    pub counts: MemoryCounts,
    pub recent_events: Vec<EventRow>,
    pub recent_facts: Vec<FactRow>,
    pub recent_preferences: Vec<PrefRow>,
    /// 最近一次预测（含反馈状态）；没有预测就是空数组。
    pub recent_predictions: Vec<PredictionRow>,
    pub last_consolidate: Option<ConsolidateRunRow>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MemoryCounts {
    pub events: i64,
    /// 按 kind 分组，count 降序（派生事件也在里面）。
    pub events_by_kind: Vec<KindCount>,
    /// 只数有效事实——被消解掉的（invalid_at 非空）不算「它现在记得的」。
    pub facts: i64,
    pub preferences: i64,
    pub boundaries: i64,
    /// 命中率的分母：所有问过的预测（含没回复的）。
    pub predictions_total: i64,
    /// 分子：feedback = 'hit'。
    pub predictions_hit: i64,
    pub tool_calls: i64,
    pub calendar_cache: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct KindCount {
    pub kind: String,
    pub count: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EventRow {
    pub id: i64,
    pub ts: String,
    pub kind: String,
    pub content: String,
    pub role: Option<String>,
    /// 派生事件（face_state / disagreement / relationship_*）：不是用户说的话，
    /// 是系统自己的状态记录。前端灰显标注。
    pub derived: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FactRow {
    pub id: i64,
    pub ts: String,
    pub content: String,
    pub confidence: f64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PrefRow {
    pub id: i64,
    pub ts: String,
    pub category: String,
    pub content: String,
    pub confidence: f64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PredictionRow {
    pub id: i64,
    pub date: String,
    pub guess: String,
    /// hit | miss | shut_up | None（还没回复）
    pub feedback: Option<String>,
    pub asked_at: Option<String>,
    pub skip_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ConsolidateRunRow {
    pub started_at: String,
    pub finished_at: Option<String>,
    pub trigger: String,
    pub status: String,
    pub extracted: i64,
    pub merged: i64,
    pub promoted: i64,
    pub consumed: i64,
}

/// limit 归一：缺省 20，下限 1，上限 100。
/// 前端传 0 当「默认」也说得过去——但 0 更可能是 bug，夹到 1 更诚实。
pub fn clamp_limit(limit: Option<u32>) -> i64 {
    match limit {
        None => DEFAULT_LIMIT,
        Some(v) => (v as i64).clamp(1, MAX_LIMIT),
    }
}

fn scalar(conn: &Connection, sql: &str) -> rusqlite::Result<i64> {
    conn.query_row(sql, [], |r| r.get(0))
}

/// 单连接只读快照。
pub fn build(conn: &Connection, limit: Option<u32>) -> rusqlite::Result<MemoryOverview> {
    let n = clamp_limit(limit);

    let mut stmt = conn.prepare(
        "SELECT kind, COUNT(*) FROM events GROUP BY kind ORDER BY COUNT(*) DESC, kind ASC",
    )?;
    let events_by_kind = stmt
        .query_map([], |row| {
            Ok(KindCount { kind: row.get(0)?, count: row.get(1)? })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    let counts = MemoryCounts {
        events: scalar(conn, "SELECT COUNT(*) FROM events")?,
        events_by_kind,
        facts: scalar(conn, "SELECT COUNT(*) FROM facts WHERE invalid_at IS NULL")?,
        preferences: scalar(conn, "SELECT COUNT(*) FROM preferences WHERE invalid_at IS NULL")?,
        boundaries: scalar(conn, "SELECT COUNT(*) FROM boundaries")?,
        predictions_total: scalar(conn, "SELECT COUNT(*) FROM predictions WHERE asked_at IS NOT NULL")?,
        // 分子必须与分母同域：没问过的预测不可能有反馈，但历史数据/测试脚本
        // 可能留下这种行，算进去命中率就会 >100%。
        predictions_hit: scalar(
            conn,
            "SELECT COUNT(*) FROM predictions WHERE feedback = 'hit' AND asked_at IS NOT NULL",
        )?,
        tool_calls: scalar(conn, "SELECT COUNT(*) FROM tool_calls")?,
        calendar_cache: scalar(conn, "SELECT COUNT(*) FROM calendar_cache")?,
    };

    let recent_events = recent_events(conn, n)?;
    let recent_facts = recent_facts(conn, n)?;
    let recent_preferences = recent_preferences(conn, n)?;
    let recent_predictions = recent_predictions(conn)?;
    let last_consolidate = last_consolidate(conn)?;

    Ok(MemoryOverview {
        counts,
        recent_events,
        recent_facts,
        recent_preferences,
        recent_predictions,
        last_consolidate,
    })
}

fn recent_events(conn: &Connection, limit: i64) -> rusqlite::Result<Vec<EventRow>> {
    // 按 id 倒序：同一毫秒内写入的事件 ts 相同，id 才是稳定顺序。
    let mut stmt = conn.prepare(
        "SELECT id, ts, kind, content, role FROM events ORDER BY id DESC LIMIT ?1",
    )?;
    let rows = stmt
        .query_map([limit], |row| {
            let kind: String = row.get(2)?;
            Ok(EventRow {
                id: row.get(0)?,
                ts: row.get(1)?,
                derived: !crate::morph::ACTIVITY_KINDS.contains(&kind.as_str()),
                kind,
                content: row.get(3)?,
                role: row.get(4)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

fn recent_facts(conn: &Connection, limit: i64) -> rusqlite::Result<Vec<FactRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, ts, content, confidence FROM facts
         WHERE invalid_at IS NULL ORDER BY id DESC LIMIT ?1",
    )?;
    let rows = stmt
        .query_map([limit], |row| {
            Ok(FactRow {
                id: row.get(0)?,
                ts: row.get(1)?,
                content: row.get(2)?,
                confidence: row.get(3)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

fn recent_preferences(conn: &Connection, limit: i64) -> rusqlite::Result<Vec<PrefRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, ts, category, content, confidence FROM preferences
         WHERE invalid_at IS NULL ORDER BY id DESC LIMIT ?1",
    )?;
    let rows = stmt
        .query_map([limit], |row| {
            Ok(PrefRow {
                id: row.get(0)?,
                ts: row.get(1)?,
                category: row.get(2)?,
                content: row.get(3)?,
                confidence: row.get(4)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

fn recent_predictions(conn: &Connection) -> rusqlite::Result<Vec<PredictionRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, date, guess, feedback, asked_at, skip_reason FROM predictions
         ORDER BY date DESC LIMIT 1",
    )?;
    let rows = stmt
        .query_map([], |row| {
            Ok(PredictionRow {
                id: row.get(0)?,
                date: row.get(1)?,
                guess: row.get(2)?,
                feedback: row.get(3)?,
                asked_at: row.get(4)?,
                skip_reason: row.get(5)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

fn last_consolidate(conn: &Connection) -> rusqlite::Result<Option<ConsolidateRunRow>> {
    let mut stmt = conn.prepare(
        "SELECT started_at, finished_at, trigger, status, extracted, merged, promoted, consumed
         FROM consolidate_runs ORDER BY id DESC LIMIT 1",
    )?;
    let mut rows = stmt.query_map([], |row| {
        Ok(ConsolidateRunRow {
            started_at: row.get(0)?,
            finished_at: row.get(1)?,
            trigger: row.get(2)?,
            status: row.get(3)?,
            extracted: row.get(4)?,
            merged: row.get(5)?,
            promoted: row.get(6)?,
            consumed: row.get(7)?,
        })
    })?;
    match rows.next() {
        Some(r) => Ok(Some(r?)),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::db;

    fn mem() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        db::init_schema_for_test(&conn).unwrap();
        conn
    }

    /// 逐表内容快照——只读性靠它证明（不是「大概没写」）。
    fn snapshot(conn: &Connection) -> Vec<(String, i64, String)> {
        let mut out = Vec::new();
        for table in [
            "events",
            "facts",
            "preferences",
            "boundaries",
            "predictions",
            "consolidate_runs",
            "settings",
            "tool_calls",
            "calendar_cache",
            "growth",
            "relationship",
        ] {
            let mut stmt = conn
                .prepare(&format!("SELECT * FROM {table} ORDER BY rowid"))
                .unwrap();
            let cols = stmt.column_count();
            let rows = stmt
                .query_map([], |row| {
                    let mut line = Vec::new();
                    for i in 0..cols {
                        let v: rusqlite::types::Value = row.get(i)?;
                        line.push(match v {
                            rusqlite::types::Value::Null => "NULL".to_string(),
                            other => format!("{other:?}"),
                        });
                    }
                    Ok(line.join("|"))
                })
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            out.push((table.to_string(), rows.len() as i64, rows.join("\n")));
        }
        out
    }

    #[test]
    fn clamp_limit_defaults_and_caps() {
        assert_eq!(clamp_limit(None), DEFAULT_LIMIT);
        assert_eq!(clamp_limit(Some(0)), 1, "0 更可能是 bug，夹到 1 而不是默认值");
        assert_eq!(clamp_limit(Some(5)), 5);
        assert_eq!(clamp_limit(Some(500)), MAX_LIMIT);
    }

    #[test]
    fn empty_db_is_all_zero_and_silent() {
        let conn = mem();
        let ov = build(&conn, None).unwrap();
        assert_eq!(ov.counts.events, 0);
        assert!(ov.counts.events_by_kind.is_empty());
        assert_eq!(ov.counts.facts, 0);
        assert_eq!(ov.counts.predictions_total, 0);
        assert_eq!(ov.counts.predictions_hit, 0);
        assert!(ov.recent_events.is_empty());
        assert!(ov.recent_facts.is_empty());
        assert!(ov.recent_preferences.is_empty());
        assert!(ov.recent_predictions.is_empty());
        assert!(ov.last_consolidate.is_none(), "没跑过整理就是 None，不是空对象");
    }

    #[test]
    fn counts_group_by_kind_and_flag_derived_events() {
        let conn = mem();
        db::add_event_meta(&conn, "dialogue", "你好", Some("user"), Some("s1")).unwrap();
        db::add_event_meta(&conn, "dialogue", "在", Some("assistant"), Some("s1")).unwrap();
        db::add_event(&conn, "face_state", "calm").unwrap();
        db::add_event(&conn, "disagreement", "没顺着说").unwrap();
        db::add_event(&conn, "relationship_stage", "R1").unwrap();

        let ov = build(&conn, None).unwrap();
        assert_eq!(ov.counts.events, 5, "派生事件也算事件（如实呈现）");
        let kinds: Vec<(&str, i64)> = ov
            .counts
            .events_by_kind
            .iter()
            .map(|k| (k.kind.as_str(), k.count))
            .collect();
        assert!(kinds.contains(&("dialogue", 2)));
        assert!(kinds.contains(&("face_state", 1)));
        assert_eq!(ov.counts.events_by_kind[0].count, 2, "count 降序");

        // 灰显标记只认 ACTIVITY_KINDS。
        let derived: Vec<(&str, bool)> = ov
            .recent_events
            .iter()
            .map(|e| (e.kind.as_str(), e.derived))
            .collect();
        assert!(derived.contains(&("dialogue", false)));
        assert!(derived.contains(&("face_state", true)));
        assert!(derived.contains(&("relationship_stage", true)));
    }

    #[test]
    fn limit_is_respected_and_invalid_facts_are_excluded() {
        let conn = mem();
        for i in 0..3 {
            db::add_fact(&conn, &format!("事实{i}"), None).unwrap();
        }
        let dead = db::add_fact(&conn, "旧事实", None).unwrap();
        db::invalidate_fact(&conn, dead).unwrap();
        for i in 0..3 {
            db::add_event(&conn, "dialogue", &format!("第{i}句")).unwrap();
        }

        let ov = build(&conn, Some(2)).unwrap();
        assert_eq!(ov.recent_events.len(), 2);
        assert_eq!(ov.recent_facts.len(), 2);
        assert_eq!(ov.counts.facts, 3, "被消解的事实不计数");
        assert!(
            ov.recent_facts.iter().all(|f| f.content != "旧事实"),
            "被消解的事实不进列表"
        );
        // 事件倒序：最后写入的在前。
        assert_eq!(ov.recent_events[0].content, "第2句");
    }

    #[test]
    fn overview_is_read_only() {
        let conn = mem();
        db::add_event(&conn, "dialogue", "x").unwrap();
        db::add_fact(&conn, "f", None).unwrap();
        db::add_preference(&conn, "habit", "h", 0.6).unwrap();
        db::add_boundary(&conn, "不准记录：密码", None).unwrap();
        db::set_setting(&conn, "llm_provider", "ollama").unwrap();

        let before = snapshot(&conn);
        let ov = build(&conn, Some(10)).unwrap();
        assert_eq!(ov.counts.events, 1);
        assert_eq!(snapshot(&conn), before, "overview 不得改动任何表");
    }

    #[test]
    fn prediction_hit_count_matches_feedback() {
        let conn = mem();
        let ask = |guess: &str, feedback: Option<&str>, asked: bool| {
            conn.execute(
                "INSERT INTO predictions (date, guess, basis, ask_after_hour, asked_at, feedback)
                 VALUES (?1, ?2, '[]', 20, ?3, ?4)",
                rusqlite::params![
                    format!("2026-11-{:02}", conn.last_insert_rowid() + 1),
                    guess,
                    if asked { Some(crate::memory::clock::now_iso()) } else { None },
                    feedback
                ],
            )
            .unwrap();
        };
        ask("带伞", Some("hit"), true);
        ask("开会", Some("hit"), true);
        ask("早睡", Some("miss"), true);
        ask("没问的", Some("hit"), false); // asked_at 为空：不计入分母

        let ov = build(&conn, None).unwrap();
        assert_eq!(ov.counts.predictions_total, 3);
        assert_eq!(ov.counts.predictions_hit, 2);
        assert_eq!(ov.recent_predictions.len(), 1, "列表只给最近一条");
    }

    #[test]
    fn forget_all_zeroes_the_overview() {
        let conn = mem();
        db::add_event(&conn, "dialogue", "x").unwrap();
        db::add_fact(&conn, "f", None).unwrap();
        db::add_preference(&conn, "like", "p", 0.6).unwrap();
        db::add_boundary(&conn, "b", None).unwrap();
        conn.execute(
            "INSERT INTO predictions (date, guess, basis, ask_after_hour, asked_at, feedback)
             VALUES ('2026-11-19', 'g', '[]', 20, '2026-11-19T10:00:00Z', 'hit')",
            [],
        )
        .unwrap();

        db::forget_all(&conn).unwrap();
        let ov = build(&conn, None).unwrap();
        assert_eq!(ov.counts.events, 0);
        assert_eq!(ov.counts.facts, 0);
        assert_eq!(ov.counts.preferences, 0);
        assert_eq!(ov.counts.boundaries, 0);
        assert_eq!(ov.counts.predictions_total, 0);
        assert!(ov.recent_predictions.is_empty());
        assert!(ov.last_consolidate.is_none());
    }
}

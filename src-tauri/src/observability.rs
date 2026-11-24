//! P4-3 / G-18 — 当日可观测时间线导出（docs/zh/plan-external-content/v1）。
//!
//! 只读聚合 turn_trace / tool_calls / security 事件 → Markdown，供用户自查。
//! 不出机、无隐式遥测。

use chrono::{Local, NaiveDate};
use rusqlite::Connection;
use serde::Serialize;

use crate::memory::db;
use crate::report::local_day_window;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct DayTimelineExport {
    pub day: String,
    pub markdown: String,
    pub turns: usize,
    pub tools: usize,
    pub security: usize,
}

/// `day` = `YYYY-MM-DD`；缺省 = 本地今天。
pub fn build_day_timeline(conn: &Connection, day: NaiveDate) -> DayTimelineExport {
    let day_s = day.format("%Y-%m-%d").to_string();
    let (start, end) = local_day_window(day)
        .unwrap_or_else(|| ("1970-01-01T00:00:00Z".into(), "2999-01-01T00:00:00Z".into()));

    let turns = turns_in_window(conn, &start, &end);
    let tools = tools_in_window(conn, &start, &end);
    let security = security_in_window(conn, &start, &end);

    let mut md = String::new();
    md.push_str(&format!("# bonsai 日时间线 · {day_s}\n\n"));
    md.push_str(&format!(
        "回合 {} · 工具调用 {} · 安全标记 {}\n\n",
        turns.len(),
        tools.len(),
        security.len()
    ));

    md.push_str("## 对话 / 决策回合\n\n");
    if turns.is_empty() {
        md.push_str("_（无）_\n\n");
    } else {
        for t in &turns {
            md.push_str(&format!(
                "- `{}` · {} · {}\n  - 用户：{}\n  - 回复：{}\n",
                short_ts(&t.ts),
                t.kind,
                t.session_id.as_deref().unwrap_or("-"),
                clip(&t.user_text, 120),
                clip(&t.reply_text, 160)
            ));
        }
        md.push('\n');
    }

    md.push_str("## 工具调用\n\n");
    if tools.is_empty() {
        md.push_str("_（无）_\n\n");
    } else {
        for r in &tools {
            md.push_str(&format!(
                "- `{}` · {}:{} · {} · {}\n",
                short_ts(&r.ts),
                r.tool,
                r.action,
                r.outcome,
                r.result_status.as_deref().unwrap_or("-")
            ));
        }
        md.push('\n');
    }

    md.push_str("## 安全 / 注入留痕\n\n");
    if security.is_empty() {
        md.push_str("_（无）_\n");
    } else {
        for s in &security {
            md.push_str(&format!("- `{}` · {}\n", short_ts(&s.ts), clip(&s.content, 200)));
        }
    }

    DayTimelineExport {
        day: day_s,
        markdown: md,
        turns: turns.len(),
        tools: tools.len(),
        security: security.len(),
    }
}

struct TurnLine {
    ts: String,
    kind: String,
    session_id: Option<String>,
    user_text: String,
    reply_text: String,
}

struct SecLine {
    ts: String,
    content: String,
}

fn turns_in_window(conn: &Connection, start: &str, end: &str) -> Vec<TurnLine> {
    let Ok(mut stmt) = conn.prepare(
        "SELECT ts, kind, session_id,
                COALESCE(user_text,''), COALESCE(reply_text,'')
           FROM turn_trace
          WHERE ts >= ?1 AND ts < ?2
          ORDER BY id ASC",
    ) else {
        return Vec::new();
    };
    stmt.query_map(rusqlite::params![start, end], |r| {
        Ok(TurnLine {
            ts: r.get(0)?,
            kind: r.get(1)?,
            session_id: r.get(2)?,
            user_text: r.get(3)?,
            reply_text: r.get(4)?,
        })
    })
    .map(|rows| rows.flatten().collect())
    .unwrap_or_default()
}

fn tools_in_window(conn: &Connection, start: &str, end: &str) -> Vec<db::ToolCallRecord> {
    // 复用审计读；再按日窗过滤（记录按最新在前，这里重排旧→新）。
    let Ok(all) = db::tool_call_records(conn, 500) else {
        return Vec::new();
    };
    let mut out: Vec<_> = all
        .into_iter()
        .filter(|r| r.ts.as_str() >= start && r.ts.as_str() < end)
        .collect();
    out.reverse();
    out
}

fn security_in_window(conn: &Connection, start: &str, end: &str) -> Vec<SecLine> {
    let Ok(mut stmt) = conn.prepare(
        "SELECT ts, content FROM events
          WHERE kind = 'security' AND ts >= ?1 AND ts < ?2
          ORDER BY id ASC",
    ) else {
        return Vec::new();
    };
    stmt.query_map(rusqlite::params![start, end], |r| {
        Ok(SecLine {
            ts: r.get(0)?,
            content: r.get(1)?,
        })
    })
    .map(|rows| rows.flatten().collect())
    .unwrap_or_default()
}

fn short_ts(ts: &str) -> &str {
    if ts.len() >= 16 {
        &ts[11..16]
    } else {
        ts
    }
}

fn clip(s: &str, n: usize) -> String {
    let one: String = s
        .chars()
        .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
        .collect();
    let mut out: String = one.chars().take(n).collect();
    if one.chars().count() > n {
        out.push('…');
    }
    if out.is_empty() {
        "—".into()
    } else {
        out
    }
}

fn parse_day(s: Option<&str>) -> Result<NaiveDate, String> {
    match s {
        None | Some("") => Ok(Local::now().date_naive()),
        Some(raw) => NaiveDate::parse_from_str(raw, "%Y-%m-%d")
            .map_err(|_| format!("bad day: {raw}（需要 YYYY-MM-DD）")),
    }
}

#[tauri::command]
pub fn observability_day_export(
    store: tauri::State<'_, crate::memory::MemoryStore>,
    day: Option<String>,
) -> Result<DayTimelineExport, String> {
    let d = parse_day(day.as_deref())?;
    store
        .with_read(|conn| Ok(build_day_timeline(conn, d)))
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::db;

    fn mem() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        db::init_schema_for_test(&c).unwrap();
        c
    }

    #[test]
    fn empty_day_still_has_sections() {
        let day = NaiveDate::from_ymd_opt(2026, 11, 24).unwrap();
        let exp = build_day_timeline(&mem(), day);
        assert_eq!(exp.day, "2026-11-24");
        assert!(exp.markdown.contains("## 对话 / 决策回合"));
        assert!(exp.markdown.contains("## 工具调用"));
        assert!(exp.markdown.contains("## 安全 / 注入留痕"));
        assert_eq!(exp.turns + exp.tools + exp.security, 0);
    }

    #[test]
    fn export_includes_security_and_tools() {
        let conn = mem();
        let day = NaiveDate::from_ymd_opt(2026, 11, 24).unwrap();
        let (start, _) = local_day_window(day).unwrap();
        // 塞进日窗内的时间戳
        conn.execute(
            "INSERT INTO events (ts, kind, content) VALUES (?1, 'security', 'injection_flagged tool=web_fetch pattern=x')",
            rusqlite::params![&start],
        )
        .unwrap();
        db::log_tool_call(&conn, "web", "fetch", "ok", Some("ok_injection_flagged"), Some(12))
            .unwrap();
        // tool_calls.ts 用 now — 若测试跑在「今天」且 day 固定 11-24 可能对不齐。
        // 直接改 ts：
        conn.execute(
            "UPDATE tool_calls SET ts = ?1",
            rusqlite::params![&start],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO turn_trace (session_id, kind, user_text, reply_text, ts)
             VALUES ('s1','model','hi','hello',?1)",
            rusqlite::params![&start],
        )
        .unwrap();

        let exp = build_day_timeline(&conn, day);
        assert_eq!(exp.security, 1);
        assert_eq!(exp.tools, 1);
        assert_eq!(exp.turns, 1);
        assert!(exp.markdown.contains("injection_flagged"));
        assert!(exp.markdown.contains("web:fetch"));
        assert!(exp.markdown.contains("用户：hi"));
    }
}

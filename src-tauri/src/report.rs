//! P0 item 6 — the one real chore: turn the day's scattered
//! clipboard items / downloads / perception signals into a single-page
//! markdown report, plus a short summary (local LLM when up, rule-based
//! fallback otherwise — the report itself never depends on the model).

use chrono::{Local, NaiveDate, TimeZone, Utc};
use rusqlite::Connection;

use crate::llm::{OllamaClient, Msg};
use crate::memory::db;

/// Local calendar day → UTC ISO window `[start, end)` matching the way
/// events are stamped (all writes go through `clock::now_iso`, UTC).
pub fn local_day_window(day: NaiveDate) -> Option<(String, String)> {
    let start = Local
        .from_local_datetime(&day.and_hms_opt(0, 0, 0)?)
        .single()?
        .with_timezone(&Utc);
    let end = start + chrono::Duration::days(1);
    Some((start.to_rfc3339(), end.to_rfc3339()))
}

/// Everything one report needs, already split by signal kind.
pub struct DayDigest {
    pub clipboards: Vec<String>,
    pub downloads: Vec<String>,
    pub apps: Vec<(String, usize)>,
    pub facts: Vec<String>,
    pub total_events: usize,
}

pub fn collect(conn: &Connection, day: NaiveDate) -> DayDigest {
    let (start, end) = match local_day_window(day) {
        Some(w) => w,
        None => return DayDigest {
            clipboards: Vec::new(),
            downloads: Vec::new(),
            apps: Vec::new(),
            facts: Vec::new(),
            total_events: 0,
        },
    };
    let events = db::events_in_window(conn, &start, &end).unwrap_or_default();
    let mut digest = DayDigest {
        clipboards: Vec::new(),
        downloads: Vec::new(),
        apps: Vec::new(),
        facts: Vec::new(),
        total_events: events.len(),
    };
    for ev in &events {
        let content = ev.content.trim();
        if let Some(rest) = content.strip_prefix("clipboard: ") {
            digest.clipboards.push(rest.to_string());
        } else if let Some(rest) = content.strip_prefix("download: ") {
            digest.downloads.push(rest.to_string());
        } else if let Some(rest) = content.strip_prefix("app: ") {
            let name = rest.to_string();
            match digest.apps.iter_mut().find(|(a, _)| *a == name) {
                Some((_, n)) => *n += 1,
                None => digest.apps.push((name, 1)),
            }
        }
    }
    digest.apps.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let facts = db::facts_in_window(conn, &start, &end).unwrap_or_default();
    digest.facts = facts.iter().map(|f| f.content.clone()).collect();
    digest
}

/// Render the markdown page. Deterministic: same data → same bytes.
pub fn render(day: NaiveDate, d: &DayDigest, summary: Option<&str>) -> String {
    let mut out = String::new();
    out.push_str(&format!("# 盆栽日报 · {day}\n\n"));

    out.push_str("## 今天\n\n");
    out.push_str(&format!(
        "- 感知事件 {} 条（剪贴板 {} · 下载 {} · 应用切换 {}）\n",
        d.total_events,
        d.clipboards.len(),
        d.downloads.len(),
        d.apps.iter().map(|(_, n)| n).sum::<usize>()
    ));
    out.push_str(&format!("- 新记忆 {} 条\n", d.facts.len()));

    if !d.clipboards.is_empty() {
        out.push_str("\n## 剪贴板\n\n");
        for c in &d.clipboards {
            out.push_str(&format!("- {c}\n"));
        }
    }
    if !d.downloads.is_empty() {
        out.push_str("\n## 下载\n\n");
        for f in &d.downloads {
            out.push_str(&format!("- {f}\n"));
        }
    }
    if !d.apps.is_empty() {
        out.push_str("\n## 活跃应用\n\n");
        for (app, n) in d.apps.iter().take(5) {
            out.push_str(&format!("- {app} ×{n}\n"));
        }
    }
    if !d.facts.is_empty() {
        out.push_str("\n## 今天学到的事\n\n");
        for f in &d.facts {
            out.push_str(&format!("- {f}\n"));
        }
    }

    out.push_str("\n## 一句话总结\n\n");
    match summary {
        Some(s) if !s.trim().is_empty() => out.push_str(&format!("> {}\n", s.trim())),
        _ => out.push_str(&fallback_summary(d)),
    }
    out
}

/// Rule-based one-liner when the local model is unavailable. Honest about
/// emptiness — a companion that invents content is worse than a quiet one.
fn fallback_summary(d: &DayDigest) -> String {
    if d.total_events == 0 && d.facts.is_empty() {
        return "> 今天没什么动静，盆栽就安静陪着。\n".into();
    }
    let mut parts = Vec::new();
    if !d.clipboards.is_empty() {
        parts.push(format!("剪贴板出现了 {} 条内容", d.clipboards.len()));
    }
    if !d.downloads.is_empty() {
        parts.push(format!("下载了 {} 个文件", d.downloads.len()));
    }
    if let Some((app, _)) = d.apps.first() {
        parts.push(format!("主要在用 {app}"));
    }
    if !d.facts.is_empty() {
        parts.push(format!("盆栽新记住了 {} 件事", d.facts.len()));
    }
    format!("> {}。\n", parts.join("，"))
}

/// Ask the local model for the summary. Any failure → None (caller falls
/// back to the rule-based line). One call, low temperature, short timeout.
pub fn llm_summary(client: &OllamaClient, day: NaiveDate, d: &DayDigest) -> Option<String> {
    if !client.is_up() {
        return None;
    }
    let mut digest = String::new();
    for c in d.clipboards.iter().take(10) {
        digest.push_str(&format!("- 剪贴板: {c}\n"));
    }
    for f in d.downloads.iter().take(10) {
        digest.push_str(&format!("- 下载: {f}\n"));
    }
    for (a, n) in d.apps.iter().take(5) {
        digest.push_str(&format!("- 使用 {a}（{n} 次记录）\n"));
    }
    for f in d.facts.iter().take(10) {
        digest.push_str(&format!("- 新记忆: {f}\n"));
    }
    if digest.is_empty() {
        digest.push_str("- （没有记录到任何活动）\n");
    }

    let prompt = format!(
        "今天是 {day}。下面是盆栽记录到的用户活动：\n{digest}\n请用一两句轻松、朴素的中文总结这一天。不要用列表、不要用标题、不要复述细节，像朋友随口聊一句。内容确实很少时，就如实说今天没什么动静。",
    );
    client
        .chat(
            &[Msg::user(prompt)],
            0.4,
            crate::llm::EXTRACT_TIMEOUT_SECS,
        )
        .ok()
        .map(|s| s.trim().to_string())
}

/// Full pipeline: collect → (optional) summarize → render → persist.
/// Returns the markdown page.
pub fn build(conn: &Connection, client: Option<&OllamaClient>, day: NaiveDate) -> String {
    let digest = collect(conn, day);
    let summary = client.and_then(|c| llm_summary(c, day, &digest));
    render(day, &digest, summary.as_deref())
}

// ---------------------------------------------------------------- command

#[tauri::command]
pub fn daily_report(
    app: tauri::AppHandle,
    store: tauri::State<'_, crate::memory::MemoryStore>,
    date: Option<String>,
) -> Result<String, String> {
    use tauri::Manager;

    let day = match date.as_deref() {
        Some(s) => NaiveDate::parse_from_str(s, "%Y-%m-%d")
            .map_err(|e| format!("bad date (want YYYY-MM-DD): {e}"))?,
        None => Local::now().date_naive(),
    };

    let conn = store.inner().0.lock().map_err(|e| e.to_string())?;
    let client = OllamaClient::from_env();
    let markdown = build(&conn, Some(&client), day);

    // Persist next to the memory db, so past reports are browsable offline.
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("reports");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!("{day}.md"));
    std::fs::write(&path, &markdown).map_err(|e| e.to_string())?;

    Ok(markdown)
}

// ---------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;

    fn mem() -> Connection {
        use std::sync::atomic::{AtomicU32, Ordering};
        static SEQ: AtomicU32 = AtomicU32::new(0);
        // Unique file per call: tests run in parallel inside one process, and
        // a shared path lets one test's rows leak into another's window.
        let dir = std::env::temp_dir().join("bonsai-report-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!(
            "test-{}-{}.sqlite3",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::SeqCst)
        ));
        db::open(&path).unwrap()
    }

    fn seed_utc(conn: &Connection, iso: &str, kind: &str, content: &str) {
        conn.execute(
            "INSERT INTO events (ts, kind, content) VALUES (?1, ?2, ?3)",
            rusqlite::params![iso, kind, content],
        )
        .unwrap();
    }

    #[test]
    fn day_window_is_utc_24h() {
        let day = NaiveDate::from_ymd_opt(2026, 11, 18).unwrap();
        let (start, end) = local_day_window(day).unwrap();
        let s = chrono::DateTime::parse_from_rfc3339(&start).unwrap();
        let e = chrono::DateTime::parse_from_rfc3339(&end).unwrap();
        assert_eq!((e - s), chrono::Duration::hours(24));
        // Start must equal local midnight converted to UTC.
        assert_eq!(
            s.with_timezone(&Local).format("%Y-%m-%d %H:%M").to_string(),
            "2026-11-18 00:00"
        );
    }

    #[test]
    fn window_filters_by_utc_timestamp() {
        let conn = mem();
        // In-window (UTC) and out-of-window events.
        let day = NaiveDate::from_ymd_opt(2026, 11, 18).unwrap();
        let (start, end) = local_day_window(day).unwrap();
        seed_utc(&conn, &start, "perception", "clipboard: in window");
        seed_utc(&conn, &end, "perception", "clipboard: exactly end (excluded)");
        seed_utc(&conn, "2026-11-10T00:00:00+00:00", "perception", "clipboard: last week");

        let events = db::events_in_window(&conn, &start, &end).unwrap();
        assert_eq!(events.len(), 1);
        assert!(events[0].content.contains("in window"));
    }

    #[test]
    fn render_skips_empty_sections_and_uses_fallback() {
        let d = DayDigest {
            clipboards: vec!["meeting notes".into()],
            downloads: vec!["paper.pdf (12345 bytes)".into()],
            apps: vec![("Code".into(), 12), ("Safari".into(), 3)],
            facts: vec!["用户喜欢雨天".into()],
            total_events: 20,
        };
        let md = render(NaiveDate::from_ymd_opt(2026, 11, 18).unwrap(), &d, None);
        assert!(md.contains("# 盆栽日报 · 2026-11-18"));
        assert!(md.contains("- meeting notes"));
        assert!(md.contains("- paper.pdf (12345 bytes)"));
        assert!(md.contains("- Code ×12"));
        assert!(md.contains("- 用户喜欢雨天"));
        assert!(md.contains("剪贴板出现了 1 条内容")); // fallback summary used
        // Sections not present when empty.
        let empty = DayDigest {
            clipboards: Vec::new(),
            downloads: Vec::new(),
            apps: Vec::new(),
            facts: Vec::new(),
            total_events: 0,
        };
        let md2 = render(NaiveDate::from_ymd_opt(2026, 11, 18).unwrap(), &empty, None);
        assert!(!md2.contains("## 剪贴板"));
        assert!(!md2.contains("## 下载"));
        assert!(md2.contains("今天没什么动静"));
    }

    #[test]
    fn explicit_summary_wins_over_fallback() {
        let d = DayDigest {
            clipboards: Vec::new(),
            downloads: Vec::new(),
            apps: Vec::new(),
            facts: Vec::new(),
            total_events: 1,
        };
        let md = render(
            NaiveDate::from_ymd_opt(2026, 11, 18).unwrap(),
            &d,
            Some("模型写的一句话"),
        );
        assert!(md.contains("模型写的一句话"));
        assert!(!md.contains("盆栽新记住了"));
    }

    #[test]
    fn build_produces_report_from_seeded_events() {
        let conn = mem();
        let day = Local::now().date_naive();
        let (start, _end) = local_day_window(day).unwrap();
        // Seed with a ts derived from the real window so it lands in-range
        // regardless of timezone.
        let mid = chrono::DateTime::parse_from_rfc3339(&start)
            .unwrap()
            + chrono::Duration::hours(1);
        seed_utc(&conn, &mid.to_rfc3339(), "perception", "clipboard: hello world");
        seed_utc(&conn, &mid.to_rfc3339(), "perception", "download: report.pdf (2048 bytes)");
        seed_utc(&conn, &mid.to_rfc3339(), "perception", "app: Code");

        let md = build(&conn, None, day);
        assert!(md.contains("- hello world"));
        assert!(md.contains("- report.pdf (2048 bytes)"));
        assert!(md.contains("- Code ×1"));
        assert!(md.contains("盆栽新记住了 0 件事") || md.contains("没什么动静") || md.contains("剪贴板出现了 1 条内容"));
    }
}

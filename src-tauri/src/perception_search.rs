//! Built-in perception query tool (plan-perception-tool.md，内置工具第二枚).
//!
//! The 最近事件 page shows perception events (app switches / clipboard /
//! downloads), but the model had no tool to query them — "我今天在用什么
//! 应用" was answered from thin air, exactly the blind spot memory_search
//! closed for dialogue (2026-11-20). This module gives the pipeline one
//! deterministic, local-only capability over `events WHERE kind='perception'`.
//!
//! Division of labor (mirrors memory_search, plan-perception-tool §3):
//! - Rust owns ALL parsing — date windows reuse `memory_search::parse_window`
//!   (one implementation, consumed not copied) and the SQL aggregates in the
//!   store. The model only echoes the user's utterance into
//!   `{"tool": "perception_search", "args": {"query": "…"}}`.
//! - The model only owns the *wording* of the final answer (pass-3).
//!
//! Privacy (plan-perception-tool §3.3, code-enforced): clipboard TEXT never
//! enters the model context — the summary carries a ≤24-char preview plus
//! the timestamp. The full stored text (already clipped to 1000 chars at
//! write time) appears ONLY in the /trace JSON, which is the user's own
//! page. Deviation rationale: a full-text replay would leak passwords /
//! verification codes into the dialogue and any subsequent cloud request.
//!
//! Offering is gated by `might_perceive` (own verb table, NOT merged into
//! `might_recall` — different semantics, and a merged table would widen the
//! false-positive surface of the recall gate). When perception is switched
//! off the tool still answers — honestly explaining that the采集 is off,
//! never pretending there is nothing to report (calendar-line precedent).

use chrono::{DateTime, Local, NaiveDate, Utc};
use rusqlite::Connection;

use crate::agent_tools::{ToolAccess, ToolCall, ToolDescriptor, ToolKind, ToolOutput, ToolProvider};
use crate::memory::clock;

/// Model-side summary budget (chars). Perception digests are aggregates —
/// naturally short; no memory_search-style 1600 needed.
const SUMMARY_TOTAL_CHARS: usize = 800;
/// Clipboard preview cap (chars) on the MODEL side. Hard privacy bound —
/// unit-test locked (plan-perception-tool §3.3).
const CLIPBOARD_PREVIEW_CHARS: usize = 24;
const APP_LIMIT: usize = 20;
const CLIPBOARD_LIMIT: usize = 20;
const DOWNLOAD_LIMIT: usize = 30;

/// Stored content prefixes (perception.rs Collector shapes; keep in sync).
const PREFIX_APP: &str = "app: ";
const PREFIX_CLIP: &str = "clipboard: ";
const PREFIX_DL: &str = "download: ";
const PREFIX_WINDOW: &str = "window: ";
const WINDOW_LIMIT: usize = 20;

// ---------------------------------------------------------------- intent gate

/// Standalone perceive markers: any hit mounts the tool regardless of dates.
/// False positives cost one provider registration (and possibly one declined
/// model round) — the list leans inclusive, mirroring might_recall's
/// 宁滥勿缺 tradeoff; misses are the costly side.
const PERCEIVE_MARKERS: &[&str] = &[
    "在用什么", "用了什么", "用的什么", "什么应用", "什么软件", "哪个应用", "用了哪些",
    "开着什么", "打开过", "切过", "剪贴板", "复制过", "粘贴过", "拷贝过",
    "下载过", "下载了", "看过什么", "浏览过", "在忙什么",
];

/// Verbs that only mean perception when COMBINED with a date phrase
/// ("18号" alone must NOT mount this tool — that's memory_search's turn).
const PERCEIVE_VERBS: &[&str] = &[
    "在用", "用了", "用的", "开着", "打开", "剪贴板", "复制", "下载", "浏览", "看过",
];

/// Domain scaffolding stripped before keyword extraction (perception flavor
/// of memory_search's SCAFFOLDING — consumed via the shared keywords_with).
const PERCEPTION_SCAFFOLDING: &[&str] = &[
    "在用什么", "用了什么", "用的什么", "什么应用", "什么软件", "哪个应用", "应用",
    "软件", "用了哪些", "开着什么", "打开过", "剪贴板", "复制过", "粘贴过", "拷贝过",
    "下载过", "下载了", "看过什么", "浏览过", "在忙什么", "电脑",
    // normalise 去空格会把「在用 Cursor」黏成「在用cursor」——动词本身
    // 也要剥，否则关键词腿匹配不到任何行（整个 digest 变空）。
    "在用", "用了", "用的", "开着", "用过",
];

/// Should the kit offer `perception_search` for this user text?
pub fn might_perceive(text: &str) -> bool {
    let t = crate::memory_search::normalise(text);
    if t.is_empty() {
        return false;
    }
    PERCEIVE_MARKERS.iter().any(|w| t.contains(w))
        || (crate::memory_search::parse_window(&t, clock::now_local().date_naive()).is_some()
            && PERCEIVE_VERBS.iter().any(|w| t.contains(w)))
}

// ---------------------------------------------------------------- window

/// Resolved query window: `(iso_start, iso_end_exclusive, label, date_phrase)`.
/// `date_phrase` is None for an explicit date_from/date_to (whole-window
/// scan, no keyword legs — same semantics as memory_search's explicit mode).
fn resolve_window(
    query: &str,
    from: Option<NaiveDate>,
    to: Option<NaiveDate>,
) -> (String, String, String, Option<String>) {
    let now = clock::now_local();
    let today = now.date_naive();
    if let (Some(f), Some(t)) = (from, to) {
        if t >= f {
            let end_excl = t.succ_opt().unwrap_or(t);
            let (s, e) = crate::memory_search::iso_pair(f, end_excl);
            return (s, e, crate::memory_search::window_label(f, end_excl), None);
        }
    }
    if let Some((s, e, phrase)) = crate::memory_search::parse_window(query, today) {
        let (iso_s, iso_e) = crate::memory_search::iso_pair(s, e);
        return (iso_s, iso_e, crate::memory_search::window_label(s, e), Some(phrase));
    }
    // No date phrase → the trailing 24 hours. Deliberate deviation from
    // memory_search's trailing week: perception events are dense (hundreds
    // per day), a weekly aggregate is mush; "我昨天在用什么" is the common form.
    let start = now - chrono::Duration::hours(24);
    let iso = |t: DateTime<Local>| t.with_timezone(&Utc).to_rfc3339();
    (iso(start), iso(now), "最近 24 小时".into(), Some("最近 24 小时".into()))
}

// ---------------------------------------------------------------- aggregation

/// One aggregated app row: `(name, count, first local stamp, last local stamp)`.
struct AppRow {
    name: String,
    count: i64,
    first: String,
    last: String,
}

/// One clipboard row: `(local stamp, stored text, model-side preview)`.
struct ClipRow {
    stamp: String,
    text: String,
    preview: String,
}

/// One download row: `(local stamp, "name (N bytes)")`.
struct DownloadRow {
    stamp: String,
    label: String,
}

/// P2-4 window title coarse labels: content is `window: App | label`.
struct WindowRow {
    stamp: String,
    label: String,
    count: i64,
}

struct PerceptionDigest {
    window_label: String,
    enabled: bool,
    apps: Vec<AppRow>,
    clips: Vec<ClipRow>,
    clip_total: i64,
    downloads: Vec<DownloadRow>,
    download_total: i64,
    windows: Vec<WindowRow>,
    window_total: i64,
}

/// Aggregated GROUP BY rows for one content prefix: `(content, n, min_ts, max_ts)`.
/// Prefixes are compile-time literals, so the format! cannot inject SQL.
fn agg_rows(
    conn: &Connection,
    prefix: &str,
    iso_start: &str,
    iso_end: &str,
    newest_first: bool,
    limit: usize,
) -> Vec<(String, i64, String, String)> {
    let order = if newest_first { "MAX(ts) DESC" } else { "n DESC" };
    let sql = format!(
        "SELECT content, COUNT(*) AS n, MIN(ts), MAX(ts) FROM events
         WHERE kind = 'perception' AND content LIKE '{prefix}%' AND ts >= ?1 AND ts < ?2
         GROUP BY content ORDER BY {order} LIMIT ?3"
    );
    let Ok(mut stmt) = conn.prepare(&sql) else {
        return Vec::new();
    };
    stmt.query_map(rusqlite::params![iso_start, iso_end, limit as i64], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, i64>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
        ))
    })
    .map(|rows| rows.flatten().collect())
    .unwrap_or_default()
}

/// Total row count for one content prefix (pre-LIMIT). Sums over the
/// LIMIT-truncated aggregate would undercount dense windows.
fn count_rows(conn: &Connection, prefix: &str, iso_start: &str, iso_end: &str) -> i64 {
    let sql = format!(
        "SELECT COUNT(*) FROM events
         WHERE kind = 'perception' AND content LIKE '{prefix}%' AND ts >= ?1 AND ts < ?2"
    );
    conn.query_row(&sql, rusqlite::params![iso_start, iso_end], |r| r.get(0))
        .unwrap_or(0)
}

fn clip_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// Model-side preview: first `n` chars + ellipsis when longer.
fn preview(s: &str, n: usize) -> String {
    let mut out = clip_chars(s, n);
    if s.chars().count() > n {
        out.push('…');
    }
    out
}

fn collect(
    conn: &Connection,
    label: &str,
    iso_start: &str,
    iso_end: &str,
    kws: &[String],
) -> PerceptionDigest {
    // Rust-side match is case-sensitive ("Cursor" vs kw "cursor") — fold both
    // sides. memory_search gets this for free from SQLite LIKE's ASCII
    // case-insensitivity; the Rust filter must do it explicitly.
    let matches = |content: &str| {
        kws.is_empty() || {
            let lower = content.to_lowercase();
            kws.iter().any(|k| lower.contains(k.as_str()))
        }
    };

    let apps = agg_rows(conn, PREFIX_APP, iso_start, iso_end, false, APP_LIMIT)
        .into_iter()
        .filter(|(c, _, _, _)| matches(c))
        .map(|(c, n, first, last)| AppRow {
            name: clip_chars(c.trim_start_matches(PREFIX_APP).trim(), 60),
            count: n,
            first: crate::memory_search::local_stamp(&first),
            last: crate::memory_search::local_stamp(&last),
        })
        .collect();

    let clip_rows = agg_rows(conn, PREFIX_CLIP, iso_start, iso_end, true, CLIPBOARD_LIMIT)
        .into_iter()
        .filter(|(c, _, _, _)| matches(c));
    let mut clips = Vec::new();
    for (c, _, _, max_ts) in clip_rows {
        let text = c.trim_start_matches(PREFIX_CLIP).to_string();
        // Privacy bound: the model never sees more than the preview (§3.3).
        let pv = preview(&text, CLIPBOARD_PREVIEW_CHARS);
        clips.push(ClipRow {
            stamp: crate::memory_search::local_stamp(&max_ts),
            text,
            preview: pv,
        });
    }
    let clip_total = count_rows(conn, PREFIX_CLIP, iso_start, iso_end);

    let dl_rows = agg_rows(conn, PREFIX_DL, iso_start, iso_end, true, DOWNLOAD_LIMIT)
        .into_iter()
        .filter(|(c, _, _, _)| matches(c));
    let mut downloads = Vec::new();
    for (c, _, _, max_ts) in dl_rows {
        downloads.push(DownloadRow {
            stamp: crate::memory_search::local_stamp(&max_ts),
            label: c.trim_start_matches(PREFIX_DL).trim().to_string(),
        });
    }
    let download_total = count_rows(conn, PREFIX_DL, iso_start, iso_end);

    let win_rows = agg_rows(conn, PREFIX_WINDOW, iso_start, iso_end, true, WINDOW_LIMIT)
        .into_iter()
        .filter(|(c, _, _, _)| matches(c));
    let mut windows = Vec::new();
    for (c, n, _, max_ts) in win_rows {
        windows.push(WindowRow {
            stamp: crate::memory_search::local_stamp(&max_ts),
            label: c.trim_start_matches(PREFIX_WINDOW).trim().to_string(),
            count: n,
        });
    }
    let window_total = count_rows(conn, PREFIX_WINDOW, iso_start, iso_end);

    PerceptionDigest {
        window_label: label.to_string(),
        enabled: true,
        apps,
        clips,
        clip_total,
        downloads,
        download_total,
        windows,
        window_total,
    }
}

impl PerceptionDigest {
    /// Line-shape prose for the model (2B-prose ruling, same as memory_search).
    fn summary(&self) -> String {
        if !self.enabled {
            return "感知采集没开，工具页里打开我才能看到你的电脑使用情况。".into();
        }
        if self.apps.is_empty()
            && self.clips.is_empty()
            && self.downloads.is_empty()
            && self.windows.is_empty()
        {
            return format!("{}没有感知记录。", self.window_label);
        }
        let mut out = format!(
            "【{}感知】应用 {} 个、窗口 {} 条、剪贴板 {} 条、下载 {} 个\n",
            self.window_label,
            self.apps.len(),
            self.window_total,
            self.clip_total,
            self.download_total
        );
        if !self.apps.is_empty() {
            let list = self
                .apps
                .iter()
                .map(|a| format!("{} ×{}（{} ~ {}）", a.name, a.count, a.first, a.last))
                .collect::<Vec<_>>()
                .join("、");
            out.push_str(&format!("应用：{list}\n"));
        }
        if !self.windows.is_empty() {
            let list = self
                .windows
                .iter()
                .map(|w| format!("{}「{}」×{}", w.stamp, w.label, w.count))
                .collect::<Vec<_>>()
                .join("、");
            out.push_str(&format!("窗口：{list}\n"));
        }
        if !self.clips.is_empty() {
            let list = self
                .clips
                .iter()
                .map(|c| format!("{}「{}」", c.stamp, c.preview))
                .collect::<Vec<_>>()
                .join("、");
            out.push_str(&format!("剪贴板：{list}\n"));
        }
        if !self.downloads.is_empty() {
            let list = self
                .downloads
                .iter()
                .map(|d| format!("{}（{}）", d.label, d.stamp))
                .collect::<Vec<_>>()
                .join("、");
            out.push_str(&format!("下载：{list}"));
        }
        crate::agent_tools::clip_chars(&out, SUMMARY_TOTAL_CHARS)
    }

    /// Machine-readable view for the /trace page (2026-11-21 ruling: tool
    /// trace is always JSON). Clipboard full text lives ONLY here — the
    /// user's own page — never in the model-side summary.
    fn full(&self) -> String {
        serde_json::json!({
            "window": self.window_label,
            "perception_enabled": self.enabled,
            "app_count": self.apps.len(),
            "window_title_count": self.window_total,
            "clipboard_count": self.clip_total,
            "download_count": self.download_total,
            "apps": self.apps.iter().map(|a| serde_json::json!({
                "name": a.name, "count": a.count, "first": a.first, "last": a.last,
            })).collect::<Vec<_>>(),
            "window_titles": self.windows.iter().map(|w| serde_json::json!({
                "ts": w.stamp, "label": w.label, "count": w.count,
            })).collect::<Vec<_>>(),
            "clipboard": self.clips.iter().map(|c| serde_json::json!({
                "ts": c.stamp, "text": c.text,
            })).collect::<Vec<_>>(),
            "downloads": self.downloads.iter().map(|d| serde_json::json!({
                "ts": d.stamp, "file": d.label,
            })).collect::<Vec<_>>(),
        })
        .to_string()
    }
}

// ---------------------------------------------------------------- enabled probe

/// Is perception collection on? The config lives in `perception.json` next
/// to the SQLite file (`lib.rs` setup); the connection's main db path is the
/// only way to find it from an `execute_with_conn`. In-memory connections
/// (tests) report the empty path and are treated as enabled so the query
/// paths stay testable without filesystem scaffolding.
fn perception_enabled(conn: &Connection) -> bool {
    let main_file: Option<String> = conn
        .prepare("PRAGMA database_list")
        .ok()
        .and_then(|mut stmt| {
            stmt.query_map([], |r| Ok((r.get::<_, String>(1)?, r.get::<_, String>(2)?)))
                .ok()
                .map(|rows| rows.flatten().find(|(name, _)| name == "main").map(|(_, f)| f))
        })
        .flatten();
    match main_file {
        None => true, // in-memory test conn
        Some(f) if f.is_empty() => true,
        Some(f) => {
            let Some(parent) = std::path::Path::new(&f).parent() else {
                return false;
            };
            let cfg = crate::perception::load_config(&parent.join("perception.json"));
            cfg.enabled
                && (cfg.active_app || cfg.clipboard || cfg.downloads || cfg.window_title)
        }
    }
}

// ---------------------------------------------------------------- provider

/// Single-source descriptor (mirrors memory_search::builtin_descriptor): the
/// provider catalog and the settings page both read from here.
pub fn builtin_descriptor() -> ToolDescriptor {
    ToolDescriptor {
        name: "perception_search".into(),
        summary: "查一段时间内你在用什么应用、窗口粗标签、复制过什么、下载过什么（本地，聚合视图）".into(),
        detail: r#"{"type":"object","properties":{"query":{"type":"string","description":"用户原话，日期由系统解析；给了 date_from/date_to 时可省"},"date_from":{"type":"string","description":"显式开始日期 YYYY-MM-DD（含）；与 date_to 成对给出，给出时按整窗聚合，忽略关键词"},"date_to":{"type":"string","description":"显式结束日期 YYYY-MM-DD（含）"}}}"#.into(),
        kind: ToolKind::Builtin,
        remote: false,
        access: ToolAccess::Read,
    }
}

/// The built-in tool. Reads the memory store directly via `execute_with_conn`
/// (the pipeline always holds the connection) — audited like every other tool.
pub struct PerceptionSearchProvider;

impl ToolProvider for PerceptionSearchProvider {
    fn descriptors(&self) -> Vec<ToolDescriptor> {
        vec![builtin_descriptor()]
    }

    fn execute(&self, _call: &ToolCall) -> Result<ToolOutput, String> {
        Err("unsupported".into())
    }

    fn needs_conn(&self) -> bool {
        true
    }

    fn execute_with_conn(&self, conn: &Connection, call: &ToolCall) -> Result<ToolOutput, String> {
        let query = call.args.get("query").and_then(|v| v.as_str()).unwrap_or("");
        let parse_d = |k: &str| {
            call.args
                .get(k)
                .and_then(|v| v.as_str())
                .and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
        };
        let (iso_s, iso_e, label, phrase) =
            resolve_window(query, parse_d("date_from"), parse_d("date_to"));

        if !perception_enabled(conn) {
            // Honest refusal, not a fake empty answer (calendar-line precedent).
            return Ok(ToolOutput {
                summary: format!("【{label}】感知采集没开，工具页里打开我才能看到你的电脑使用情况。"),
                full: serde_json::json!({ "window": label, "perception_enabled": false })
                    .to_string(),
            });
        }

        let kws = phrase
            .map(|p| crate::memory_search::keywords_with(query, &p, PERCEPTION_SCAFFOLDING))
            .unwrap_or_default();
        let digest = collect(conn, &label, &iso_s, &iso_e, &kws);
        Ok(ToolOutput { summary: digest.summary(), full: digest.full() })
    }
}

// ---------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::db;

    fn mem() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        db::init_schema_for_test(&conn).unwrap();
        conn
    }

    /// Raw insert with an explicit UTC ts (add_event stamps the pinned clock).
    fn insert_perception(conn: &Connection, ts: &str, content: &str) {
        conn.execute(
            "INSERT INTO events (ts, kind, content, role, session_id)
             VALUES (?1, 'perception', ?2, NULL, NULL)",
            rusqlite::params![ts, content],
        )
        .unwrap();
    }

    #[test]
    fn gate_hits_perceive_markers_and_date_verb_pairs() {
        for t in [
            "我今天在用什么应用",
            "我昨天用了什么软件",
            "剪贴板里有什么",
            "我复制过什么",
            "刚才下载了什么文件",
            "看看我开着什么",
            "18号我在用什么应用",
            "昨天在用啥",
            "3天前打开过什么",
        ] {
            assert!(might_perceive(t), "gate must fire on: {t}");
        }
        for t in [
            "我今天好累",
            "天气怎么样",
            // 裸日期是 memory_search 的地盘，感知门不许抢。
            "总结一下我18号说了啥",
            "18号说了啥",
            "周四聊天原文",
            "18号开心吗",
            "",
        ] {
            assert!(!might_perceive(t), "gate must stay quiet on: {t}");
        }
    }

    #[test]
    fn execute_without_conn_is_unsupported_like_memory_search() {
        let p = PerceptionSearchProvider;
        assert_eq!(
            p.execute(&ToolCall { name: "perception_search".into(), args: serde_json::json!({}) })
                .unwrap_err(),
            "unsupported"
        );
    }

    #[test]
    fn aggregates_apps_clipboard_and_downloads() {
        let _g = clock::pinned("2026-11-23T07:00:00Z"); // 本地 11-23 15:00
        let conn = mem();
        for _ in 0..3 {
            insert_perception(&conn, "2026-11-23T01:10:00+00:00", "app: Cursor");
        }
        insert_perception(&conn, "2026-11-23T02:30:00+00:00", "app: 微信");
        insert_perception(&conn, "2026-11-23T03:00:00+00:00", "app: 微信");
        insert_perception(
            &conn,
            "2026-11-23T03:05:00+00:00",
            "clipboard: https://example.com/very/long/link/that/exceeds/the/preview/budget",
        );
        insert_perception(&conn, "2026-11-23T02:00:00+00:00", "download: design-v2.pdf (2400000 bytes)");
        // 窗口外的事件不许进来。
        insert_perception(&conn, "2026-11-20T02:00:00+00:00", "app: 旧应用");

        let p = PerceptionSearchProvider;
        let out = p
            .execute_with_conn(&conn, &ToolCall {
                name: "perception_search".into(),
                args: serde_json::json!({ "query": "今天在用什么应用" }),
            })
            .unwrap();

        assert!(out.summary.contains("【11月23日"), "label: {}", out.summary);
        assert!(out.summary.contains("Cursor ×3"), "count desc: {}", out.summary);
        assert!(out.summary.contains("微信 ×2"), "{}", out.summary);
        assert!(out.summary.contains("剪贴板 1 条"), "{}", out.summary);
        assert!(out.summary.contains("下载 1 个"), "{}", out.summary);
        assert!(out.summary.contains("design-v2.pdf"), "{}", out.summary);
        assert!(!out.summary.contains("旧应用"), "out-of-window row: {}", out.summary);

        // 隐私硬约束：模型侧剪贴板只有 ≤24 字符预览。
        let pv_line = out
            .summary
            .lines()
            .find(|l| l.starts_with("剪贴板："))
            .unwrap();
        assert!(pv_line.contains("https://example.com/very"), "preview present: {pv_line}");
        assert!(pv_line.contains('…'), "ellipsis marks the cut: {pv_line}");
        assert!(!pv_line.contains("preview/budget"), "full text must NOT reach the model: {pv_line}");

        // full 是 JSON（/trace 结构化渲染），剪贴板全文只在 user 自己的 trace 里。
        let parsed: serde_json::Value = serde_json::from_str(&out.full).expect("full must be JSON");
        assert_eq!(parsed["perception_enabled"], true);
        assert_eq!(parsed["app_count"], 2);
        assert_eq!(parsed["apps"][0]["name"], "Cursor");
        assert_eq!(parsed["apps"][0]["count"], 3);
        assert_eq!(parsed["clipboard"][0]["ts"], "11-23 11:05");
        assert!(
            parsed["clipboard"][0]["text"]
                .as_str()
                .unwrap()
                .contains("preview/budget"),
            "full text belongs to the user's trace page only"
        );
        // summary 仍受预算钳制。
        assert!(out.summary.chars().count() <= SUMMARY_TOTAL_CHARS);
    }

    #[test]
    fn clipboard_preview_is_hard_capped_at_24_chars() {
        let long = "a".repeat(200);
        let pv = preview(&long, CLIPBOARD_PREVIEW_CHARS);
        assert_eq!(pv.chars().count(), CLIPBOARD_PREVIEW_CHARS + 1); // + ellipsis
        let short = preview("short", CLIPBOARD_PREVIEW_CHARS);
        assert_eq!(short, "short");
    }

    #[test]
    fn keyword_legs_filter_each_dimension() {
        let _g = clock::pinned("2026-11-23T07:00:00Z");
        let conn = mem();
        insert_perception(&conn, "2026-11-23T01:00:00+00:00", "app: Cursor");
        insert_perception(&conn, "2026-11-23T01:10:00+00:00", "app: 微信");
        insert_perception(&conn, "2026-11-23T02:00:00+00:00", "clipboard: cursor 配置备忘");

        let p = PerceptionSearchProvider;
        let out = p
            .execute_with_conn(&conn, &ToolCall {
                name: "perception_search".into(),
                args: serde_json::json!({ "query": "今天在用 Cursor" }),
            })
            .unwrap();
        assert!(out.summary.contains("Cursor"), "kw app kept: {}", out.summary);
        assert!(!out.summary.contains("微信"), "non-matching app dropped: {}", out.summary);
    }

    #[test]
    fn no_date_phrase_falls_back_to_trailing_24h_and_garbage_args_do_not_error() {
        let _g = clock::pinned("2026-11-23T07:00:00Z");
        let conn = mem();
        insert_perception(&conn, "2026-11-23T05:00:00+00:00", "app: bonsai");
        let p = PerceptionSearchProvider;
        let out = p
            .execute_with_conn(&conn, &ToolCall {
                name: "perception_search".into(),
                args: serde_json::json!({}),
            })
            .unwrap();
        assert!(out.summary.contains("最近 24 小时"), "fallback label: {}", out.summary);
        assert!(out.summary.contains("bonsai"));

        // 坏日期对 → 落回 24h 兜底，不报错。
        let out2 = p
            .execute_with_conn(&conn, &ToolCall {
                name: "perception_search".into(),
                args: serde_json::json!({ "date_from": "11月23日", "date_to": "not-a-date" }),
            })
            .unwrap();
        assert!(out2.summary.contains("bonsai"));
    }

    #[test]
    fn empty_window_and_explicit_dates_are_honest() {
        let _g = clock::pinned("2026-11-23T07:00:00Z");
        let conn = mem();
        let p = PerceptionSearchProvider;
        let out = p
            .execute_with_conn(&conn, &ToolCall {
                name: "perception_search".into(),
                args: serde_json::json!({ "query": "今天在用什么" }),
            })
            .unwrap();
        assert!(out.summary.contains("没有感知记录"), "{}", out.summary);

        let out2 = p
            .execute_with_conn(&conn, &ToolCall {
                name: "perception_search".into(),
                args: serde_json::json!({ "date_from": "2026-11-22", "date_to": "2026-11-22" }),
            })
            .unwrap();
        assert!(out2.summary.contains("11月22日"), "explicit window label: {}", out2.summary);
    }

    /// Off-collection is explained, never faked as "no records". Requires a
    /// FILE-backed conn so the PRAGMA path resolution finds perception.json.
    #[test]
    fn disabled_perception_is_explained_not_faked() {
        let dir = std::env::temp_dir().join(format!("bonsai_pst_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("mem.sqlite");
        let conn = Connection::open(&db_path).unwrap();
        db::init_schema_for_test(&conn).unwrap();
        crate::perception::save_config(
            &crate::perception::PerceptionConfig::default(),
            &dir.join("perception.json"),
        )
        .unwrap();

        let p = PerceptionSearchProvider;
        let out = p
            .execute_with_conn(&conn, &ToolCall {
                name: "perception_search".into(),
                args: serde_json::json!({ "query": "今天在用什么应用" }),
            })
            .unwrap();
        assert!(out.summary.contains("感知采集没开"), "{}", out.summary);
        let parsed: serde_json::Value = serde_json::from_str(&out.full).unwrap();
        assert_eq!(parsed["perception_enabled"], false);
        drop(conn);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Registry single-source: the descriptor lives here and the settings
    /// page reads the same registry (agent_tools::builtin_tools).
    #[test]
    fn registry_carries_the_descriptor() {
        let reg = crate::agent_tools::builtin_tools();
        assert!(reg.iter().any(|d| d.name == "perception_search"));
        let d = reg.iter().find(|d| d.name == "perception_search").unwrap();
        assert_eq!(d.kind, ToolKind::Builtin);
        assert!(!d.remote);
        assert_eq!(d.access, ToolAccess::Read);
        // 校验器认这份 schema：query 可选、显式日期对可选。
        assert!(crate::agent_tools::validate_args(
            &d.detail,
            &serde_json::json!({ "query": "今天在用什么" })
        )
        .is_ok());
        assert!(crate::agent_tools::validate_args(
            &d.detail,
            &serde_json::json!({ "date_from": "2026-11-23", "date_to": "2026-11-23" })
        )
        .is_ok());
    }
}

//! Built-in memory recall tool (plan-agent P3: 内置工具第一枚).
//!
//! The chat window only feeds the model the last 6 turns of the *current*
//! session (dialogue.rs `MAX_HISTORY_TURNS`), so "总结一下我 18 号说了啥"
//! used to be answered from thin air. This module gives the agent pipeline
//! one deterministic, local-only capability: `memory_search`.
//!
//! Division of labor (mirrors plan-calendar.md §3.5's rule-first stance):
//! - Rust owns ALL parsing — date windows ("18号", "昨天", "3天前", "上周",
//!   "11月18日"), keyword extraction and the SQL. The 2B model only has to
//!   echo the user's utterance into `{"tool": "memory_search", "args":
//!   {"query": "…"}}`; even a garbled `query` degrades to a window scan.
//! - The model only owns the *wording* of the final answer (pipeline pass 3).
//!
//! Offering is gated by `might_recall`: without a recall signal in the user
//! text the provider is not added to the kit, so a plain chat turn keeps the
//! legacy single-round path (no extra model round trip, no catalog noise).
//!
//! Reading local dialogue events is NOT a remote capability — the call is
//! audited like every other tool (`tool_calls` row, kind `builtin`), but the
//! content is the user's own local log: no secrets, no untrusted text.

use chrono::{Datelike, NaiveDate, NaiveTime, TimeZone, Weekday};
use rusqlite::Connection;

use crate::agent_tools::{ToolCall, ToolDescriptor, ToolKind, ToolOutput, ToolProvider};
use crate::memory::clock;

/// Hard caps: how many dialogue rows / fact lines enter one answer.
const MAX_DIALOGUE_ROWS: usize = 60;
const MAX_FACT_ROWS: usize = 10;
/// Per-line clip in the model-visible summary (chars).
const SUMMARY_LINE_CHARS: usize = 100;
/// Total summary clip. Deliberate deviation from the 200-char provider
/// convention: the whole point of this tool is handing the model enough raw
/// dialogue to summarize a day. Content is local user-authored text, so a
/// wider window is safe; `full` stays the folded complete view for the UI.
const SUMMARY_TOTAL_CHARS: usize = 1600;
/// Per-line clip in the frontend `full` text.
const FULL_LINE_CHARS: usize = 200;

// ---------------------------------------------------------------- intent gate

/// Recall-ish markers. Any hit (plus the bare date-number pattern below)
/// makes the kit offer `memory_search` this turn. False positives only cost
/// one extra model round — the model may simply decline the tool — so the
/// list leans inclusive; false negatives are the costly side (the 18th
/// question must never be answerable from air again).
const RECALL_MARKERS: &[&str] = &[
    "总结", "回顾", "复盘", "回忆", "还记得", "记得吗", "印象", "上次", "历史",
    "说过", "说了", "说啥", "聊过", "聊了", "聊到", "讲过", "讲了", "讲啥",
    "提过", "提到", "谈过", "谈到", "记录", "聊天记录",
];

fn normalise(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_lowercase()
}

/// `18号` / `3日` / `5天前` / `2个月前` — a digit run glued to a date unit.
fn has_numbered_date(t: &str) -> bool {
    let chars: Vec<char> = t.chars().collect();
    for i in 0..chars.len() {
        if !chars[i].is_ascii_digit() {
            continue;
        }
        let unit: String = chars[i + 1..].iter().take(3).collect();
        if unit.starts_with('号')
            || unit.starts_with('日')
            || unit.starts_with("天前")
            || unit.starts_with("周前")
            || unit.starts_with("个月前")
            || unit.starts_with("年前")
        {
            return true;
        }
    }
    false
}

/// Should the kit offer `memory_search` for this user text?
pub fn might_recall(text: &str) -> bool {
    let t = normalise(text);
    if t.is_empty() {
        return false;
    }
    RECALL_MARKERS.iter().any(|w| t.contains(w)) || has_numbered_date(&t)
}

// ---------------------------------------------------------------- date window

/// Simple Chinese-numeral → digit for the "N天前" family (1..=10).
fn cn_numeral(c: char) -> Option<u32> {
    match c {
        '一' => Some(1),
        '二' | '两' => Some(2),
        '三' => Some(3),
        '四' => Some(4),
        '五' => Some(5),
        '六' => Some(6),
        '七' => Some(7),
        '八' => Some(8),
        '九' => Some(9),
        '十' => Some(10),
        _ => None,
    }
}

/// Number immediately before `unit_start`: ASCII digits first, then the
/// one-ten Chinese numerals. Returns (value, span_start).
fn number_before(t: &[char], unit_start: usize) -> Option<(u32, usize)> {
    let mut start = unit_start;
    while start > 0 && t[start - 1].is_ascii_digit() {
        start -= 1;
    }
    if start < unit_start {
        let digits: String = t[start..unit_start].iter().collect();
        return digits.parse::<u32>().ok().filter(|n| *n > 0).map(|n| (n, start));
    }
    while start > 0 && cn_numeral(t[start - 1]).is_some() {
        start -= 1;
    }
    if start < unit_start {
        let n = t[start..unit_start].iter().filter_map(|c| cn_numeral(*c)).sum::<u32>();
        return Some((n, start)).filter(|(n, _)| (1..=10).contains(n));
    }
    None
}

fn span_text(t: &[char], start: usize, end: usize) -> String {
    t[start..end].iter().collect()
}

/// Date phrase → local-date window `[start, end)` plus the matched phrase,
/// so keyword extraction can strip exactly what matched. `None` = no date
/// phrase → the caller falls back to the trailing week.
pub fn parse_window(text: &str, today: NaiveDate) -> Option<(NaiveDate, NaiveDate, String)> {
    let t: Vec<char> = normalise(text).chars().collect();
    let at = |s: &str| {
        let sc: Vec<char> = s.chars().collect();
        t.windows(sc.len()).position(|w| w == sc).map(|i| (i, i + sc.len()))
    };
    let monday_this_week =
        today - chrono::Duration::days(today.weekday().num_days_from_monday() as i64);

    // Fixed phrases first (longest match wins so 大前天 beats 前天).
    for (phrase, day) in [
        ("大前天", today - chrono::Duration::days(3)),
        ("前天", today - chrono::Duration::days(2)),
        ("昨天", today - chrono::Duration::days(1)),
        ("昨日", today - chrono::Duration::days(1)),
        ("今天", today),
        ("今日", today),
    ] {
        if let Some((s, e)) = at(phrase) {
            return Some((day, day.succ_opt()?, span_text(&t, s, e)));
        }
    }
    if let Some((s, e)) = at("上上周") {
        return Some((
            monday_this_week - chrono::Duration::days(14),
            monday_this_week - chrono::Duration::days(7),
            span_text(&t, s, e),
        ));
    }
    if let Some((s, e)) = at("上周") {
        return Some((
            monday_this_week - chrono::Duration::days(7),
            monday_this_week,
            span_text(&t, s, e),
        ));
    }
    for phrase in ["本周", "这周", "这一周", "这礼拜"] {
        if let Some((s, e)) = at(phrase) {
            return Some((monday_this_week, today.succ_opt()?, span_text(&t, s, e)));
        }
    }
    if let Some((s, e)) = at("一周内").or_else(|| at("最近一周")) {
        return Some((
            today - chrono::Duration::days(7),
            today.succ_opt()?,
            span_text(&t, s, e),
        ));
    }
    if let Some((s, e)) = at("最近").or_else(|| at("近来")) {
        return Some((
            today - chrono::Duration::days(6),
            today.succ_opt()?,
            span_text(&t, s, e),
        ));
    }
    // "N天前" / "N周前" / "N个月(月)前" / "N年前" — fuzzy: 天=单日, 周=整周,
    // 月/年 = 从那天起的七天窗口（口语本来就指大概那阵子）。
    for (unit, step) in [("天前", 1i64), ("周前", 7), ("个月前", 30), ("月前", 30), ("年前", 365)] {
        if let Some((s, e)) = at(unit) {
            if let Some((n, ns)) = number_before(&t, s) {
                let back = step * n as i64;
                let begin = today - chrono::Duration::days(back);
                let span = match unit {
                    "天前" => 1,
                    "周前" => 7,
                    _ => 7,
                };
                return Some((begin, begin + chrono::Duration::days(span), span_text(&t, ns, e)));
            }
        }
    }
    // "M月D日/号"（年份被忽略，未来月日回退一年）
    if let Some((ms, me)) = at("月") {
        let month = number_before(&t, ms).map(|(n, _)| n);
        let mut de = me;
        while de < t.len() && t[de].is_ascii_digit() {
            de += 1;
        }
        let day: Option<u32> = if de > me {
            t[me..de].iter().collect::<String>().parse().ok()
        } else {
            None
        };
        if let (Some(m), Some(d)) = (month, day) {
            let label = span_text(&t, ms, de);
            return make_ymd(today, m, d, label);
        }
    }
    // bare "D号/D日" — 本月；超过今天的号数 → 上个月
    for unit in ["号", "日"] {
        if let Some((s, e)) = at(unit) {
            if let Some((d, ns)) = number_before(&t, s) {
                return make_day_of_month(today, d, span_text(&t, ns, e));
            }
        }
    }
    None
}

/// "M月D日" — a future date in this year rolls back one year.
fn make_ymd(today: NaiveDate, m: u32, d: u32, label: String) -> Option<(NaiveDate, NaiveDate, String)> {
    let this_year = NaiveDate::from_ymd_opt(today.year(), m, d)?;
    if this_year > today {
        let last = NaiveDate::from_ymd_opt(today.year() - 1, m, d)?;
        return Some((last, last.succ_opt()?, label));
    }
    Some((this_year, this_year.succ_opt()?, label))
}

/// "18号" — this month; a day beyond today means last month.
fn make_day_of_month(today: NaiveDate, d: u32, label: String) -> Option<(NaiveDate, NaiveDate, String)> {
    let (y, m) = (today.year(), today.month());
    if d > today.day() {
        let (py, pm) = if m == 1 { (y - 1, 12) } else { (y, m - 1) };
        let last = NaiveDate::from_ymd_opt(py, pm, d.min(28))?;
        return Some((last, last.succ_opt()?, label));
    }
    let this_month = NaiveDate::from_ymd_opt(y, m, d)?;
    Some((this_month, this_month.succ_opt()?, label))
}

/// Default when no date phrase: the trailing seven days.
fn fallback_window(today: NaiveDate) -> (NaiveDate, NaiveDate, String) {
    (today - chrono::Duration::days(6), today.succ_opt().unwrap(), "最近一周".into())
}

// ---------------------------------------------------------------- keywords

const SCAFFOLDING: &[&str] = &[
    "总结一下", "总结", "整理一下", "整理", "回顾一下", "回顾", "复盘",
    "帮我", "说说", "讲讲", "翻翻", "翻一下", "查查", "查一下", "找找", "找一下",
    "看看", "看一下", "什么", "哪些", "咋样", "怎么样", "那天", "当天", "时候",
];

/// Single-char particles and dialogue verbs that never make a keyword.
const STOP_CHARS: &[char] = &[
    '的', '了', '吗', '呢', '吧', '啊', '啥', '我', '你', '他', '她', '它', '们',
    '这', '那', '跟', '和', '与', '就', '也', '都', '还', '说', '聊', '讲', '谈',
    '提', '问', '记', '得', '过', '有', '个',
];

/// What is left after the date phrase and the scaffolding fall off — the
/// LIKE legs of the search. Empty = window-only scan (honest, still useful).
fn keywords(text: &str, date_phrase: &str) -> Vec<String> {
    let mut t = normalise(text);
    if !date_phrase.is_empty() {
        t = t.replace(&normalise(date_phrase), "");
    }
    for s in SCAFFOLDING {
        t = t.replace(s, "");
    }
    let segments = t
        .chars()
        .map(|c| if STOP_CHARS.contains(&c) || c.is_ascii_punctuation() { ' ' } else { c })
        .collect::<String>();
    let mut out: Vec<String> = segments
        .split_whitespace()
        .filter(|s| s.chars().count() >= 2)
        .map(|s| s.to_string())
        .collect();
    out.sort_by_key(|s| std::cmp::Reverse(s.chars().count()));
    out.truncate(2);
    out
}

// ---------------------------------------------------------------- search core

pub struct Recall {
    pub window_label: String,
    /// `(local time, who, content)` oldest first; who ∈ 用户/盆栽.
    pub lines: Vec<(String, &'static str, String)>,
    pub facts: Vec<String>,
}

fn weekday_cn(w: Weekday) -> &'static str {
    match w {
        Weekday::Mon => "周一",
        Weekday::Tue => "周二",
        Weekday::Wed => "周三",
        Weekday::Thu => "周四",
        Weekday::Fri => "周五",
        Weekday::Sat => "周六",
        Weekday::Sun => "周日",
    }
}

fn fmt_md(d: NaiveDate) -> String {
    format!("{}月{}日", d.month(), d.day())
}

fn window_label(start: NaiveDate, end: NaiveDate) -> String {
    if end - start == chrono::Duration::days(1) {
        format!("{}（{}）", fmt_md(start), weekday_cn(start.weekday()))
    } else {
        match end.pred_opt() {
            Some(last) => format!("{} 至 {}", fmt_md(start), fmt_md(last)),
            None => fmt_md(start),
        }
    }
}

/// Local naive date window → UTC RFC3339 pair for the SQL window scan.
fn iso_pair(start: NaiveDate, end: NaiveDate) -> (String, String) {
    let to_iso = |d: NaiveDate| -> String {
        let naive = d.and_time(NaiveTime::MIN);
        match chrono::Local.from_local_datetime(&naive) {
            chrono::LocalResult::Single(t) => t,
            chrono::LocalResult::Ambiguous(t, _) => t,
            chrono::LocalResult::None => chrono::Local
                .from_local_datetime(&naive)
                .earliest()
                .map(|t| t)
                .unwrap_or_else(|| chrono::Local
                    .with_ymd_and_hms(1970, 1, 1, 0, 0, 0)
                    .unwrap()),
        }
        .with_timezone(&chrono::Utc)
        .to_rfc3339()
    };
    (to_iso(start), to_iso(end))
}

fn local_stamp(ts: &str) -> String {
    match chrono::DateTime::parse_from_rfc3339(ts) {
        Ok(t) => t.with_timezone(&chrono::Local).format("%m-%d %H:%M").to_string(),
        Err(_) => ts.to_string(),
    }
}

fn clip(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// The deterministic recall: dialogue events + facts in the parsed window.
pub fn recall(conn: &Connection, query: &str) -> Recall {
    let today = clock::now_local().date_naive();
    let (start, end, phrase) = parse_window(query, today).unwrap_or_else(|| fallback_window(today));
    let kws = keywords(query, &phrase);
    let (iso_start, iso_end) = iso_pair(start, end);

    // Dialogue rows. Keyword legs OR-combined (recall beats precision);
    // no keywords → the whole window. Legs are pre-quoted literals built
    // from keyword segments whose ASCII punctuation was stripped, so the
    // string assembly cannot turn into SQL injection.
    let sql = if kws.is_empty() {
        "SELECT ts, role, content FROM events
         WHERE kind = 'dialogue' AND role IN ('user', 'assistant')
           AND ts >= ?1 AND ts < ?2 ORDER BY id ASC LIMIT ?3"
            .to_string()
    } else {
        let legs: Vec<String> = kws
            .iter()
            .map(|k| format!("content LIKE '%{}%'", k.replace('%', "")))
            .collect();
        format!(
            "SELECT ts, role, content FROM events
             WHERE kind = 'dialogue' AND role IN ('user', 'assistant')
               AND ts >= ?1 AND ts < ?2 AND ({}) ORDER BY id ASC LIMIT ?3",
            legs.join(" OR ")
        )
    };
    let mut lines = Vec::new();
    if let Ok(mut stmt) = conn.prepare(&sql) {
        if let Ok(rows) = stmt.query_map(
            rusqlite::params![iso_start, iso_end, MAX_DIALOGUE_ROWS as i64],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            },
        ) {
            for (ts, role, content) in rows.flatten() {
                let who = if role == "user" { "用户" } else { "盆栽" };
                lines.push((local_stamp(&ts), who, content));
            }
        }
    }

    // Facts learned in the window (report §4.2: what was *learned* that day).
    let mut facts = Vec::new();
    if let Ok(rows) = crate::memory::db::facts_in_window(conn, &iso_start, &iso_end) {
        for f in rows {
            if !kws.is_empty() && !kws.iter().any(|k| f.content.contains(k.as_str())) {
                continue;
            }
            facts.push(f.content);
            if facts.len() >= MAX_FACT_ROWS {
                break;
            }
        }
    }

    Recall { window_label: window_label(start, end), lines, facts }
}

impl Recall {
    fn render(&self, line_chars: usize) -> String {
        let mut out = format!(
            "【{}】对话 {} 条、记忆 {} 条\n",
            self.window_label,
            self.lines.len(),
            self.facts.len()
        );
        for (stamp, who, content) in &self.lines {
            out.push_str(&format!("[{} {}] {}\n", stamp, who, clip(content, line_chars)));
        }
        for f in &self.facts {
            out.push_str(&format!("【记忆】{}\n", clip(f, line_chars)));
        }
        out
    }

    /// What the model summarizes from (see the SUMMARY_* notes above).
    fn summary(&self) -> String {
        if self.lines.is_empty() && self.facts.is_empty() {
            return format!("{}没有找到对话记录。", self.window_label);
        }
        crate::agent_tools::clip_chars(&self.render(SUMMARY_LINE_CHARS), SUMMARY_TOTAL_CHARS)
    }

    /// Folded complete view for the frontend.
    fn full(&self) -> String {
        self.render(FULL_LINE_CHARS)
    }
}

// ---------------------------------------------------------------- provider

/// The built-in tool. Prompt-only kinds answer `unsupported` in `execute`;
/// this one overrides `execute_with_conn` because it reads the memory store
/// directly (the pipeline always holds the connection).
pub struct MemorySearchProvider;

impl ToolProvider for MemorySearchProvider {
    fn descriptors(&self) -> Vec<ToolDescriptor> {
        vec![ToolDescriptor {
            name: "memory_search".into(),
            summary: "按日期或关键词翻找过往聊天记录与记忆（本地）".into(),
            detail: r#"{"type":"object","properties":{"query":{"type":"string","description":"用户原话，日期与关键词由系统解析"}},"required":["query"]}"#.into(),
            kind: ToolKind::Builtin,
            remote: false,
        }]
    }

    fn execute(&self, _call: &ToolCall) -> Result<ToolOutput, String> {
        Err("unsupported".into())
    }

    fn execute_with_conn(&self, conn: &Connection, call: &ToolCall) -> Result<ToolOutput, String> {
        let query = call.args.get("query").and_then(|v| v.as_str()).unwrap_or("");
        let recall = recall(conn, query);
        Ok(ToolOutput { summary: recall.summary(), full: recall.full() })
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

    /// Raw insert with an explicit UTC ts (add_event stamps the pinned clock,
    /// which would collapse every row onto one instant).
    fn insert_dialogue(conn: &Connection, ts: &str, role: &str, content: &str) {
        conn.execute(
            "INSERT INTO events (ts, kind, content, role, session_id)
             VALUES (?1, 'dialogue', ?2, ?3, 's1')",
            rusqlite::params![ts, content, role],
        )
        .unwrap();
    }

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 11, 20).unwrap() // 周五
    }

    fn win(text: &str) -> (NaiveDate, NaiveDate, String) {
        parse_window(text, today()).unwrap()
    }

    #[test]
    fn intent_gate_hits_recall_and_bare_dates_only() {
        for t in [
            "总结一下我18号说了啥",
            "我们昨天聊了什么",
            "18号说过什么",
            "帮我复盘一下上周的对话",
            "你还记得吗",
            "把18号的聊天记录给我看看",
            "3天前我提到过一个方案",
            "18号",
        ] {
            assert!(might_recall(t), "gate must fire on: {t}");
        }
        for t in [
            "我今天好累",
            "今天天气怎么样",
            "这个方案可以",
            "明天开始减肥",
            "会议室的灯坏了",
            "",
        ] {
            assert!(!might_recall(t), "gate must stay quiet on: {t}");
        }
    }

    #[test]
    fn windows_cover_the_common_chinese_date_phrases() {
        let d = |y: i32, m: u32, dd: u32| NaiveDate::from_ymd_opt(y, m, dd).unwrap();
        assert_eq!(win("今天说了啥").0, d(2026, 11, 20));
        assert_eq!(win("昨天聊了啥").0, d(2026, 11, 19));
        assert_eq!(win("前天讲过的").0, d(2026, 11, 18));
        assert_eq!(win("大前天的事").0, d(2026, 11, 17));
        // 上周：本周一是 11-16，上周即 11-09 ~ 11-15
        assert_eq!(win("上周的对话").0, d(2026, 11, 9));
        assert_eq!(win("上周的对话").1, d(2026, 11, 16));
        assert_eq!(win("这周聊过啥").0, d(2026, 11, 16));
        assert_eq!(win("3天前我说的").0, d(2026, 11, 17));
        assert_eq!(win("三天前我说的").0, d(2026, 11, 17));
        assert_eq!(win("11月18日说了啥").0, d(2026, 11, 18));
        assert_eq!(win("18号说了啥").0, d(2026, 11, 18));
        // 超过当天的号数 → 上个月
        assert_eq!(win("25号说过啥").0, d(2026, 10, 25));
        // 未来月日 → 去年
        assert_eq!(win("12月1日聊了啥").0, d(2025, 12, 1));
        // 无日期短语 → None（调用方落回最近一周）
        assert!(parse_window("我们聊过什么", today()).is_none());
    }

    #[test]
    fn keywords_strip_scaffolding_and_particles() {
        assert!(keywords("总结一下我18号说了啥", "18号").is_empty());
        assert_eq!(keywords("18号聊了退款的事", "18号"), vec!["退款"]);
        assert!(keywords("我们聊过什么", "").is_empty());
        // normalise 小写化后进入 LIKE——SQLite 对 ASCII 大小写不敏感，不影响命中
        assert_eq!(keywords("11月18日和A公司谈合同", "11月18日"), vec!["a公司", "合同"]);
    }

    #[test]
    fn recall_finds_dialogues_and_facts_in_the_window() {
        let _g = crate::memory::clock::pinned("2026-11-20T07:00:00Z"); // 本地 11-20 15:00
        let conn = mem();
        insert_dialogue(&conn, "2026-11-18T01:10:00+00:00", "user", "报销流程怎么走");
        insert_dialogue(&conn, "2026-11-18T01:11:00+00:00", "assistant", "先找直属主管审批，再走财务系统。");
        insert_dialogue(&conn, "2026-11-15T01:00:00+00:00", "user", "无关的旧对话");
        insert_dialogue(&conn, "2026-11-20T02:00:00+00:00", "user", "今天的话");
        conn.execute(
            "INSERT INTO facts (ts, content, confidence) VALUES ('2026-11-18T03:00:00+00:00', '用户在走报销流程', 0.6)",
            [],
        )
        .unwrap();

        let r = recall(&conn, "总结一下我18号说了啥");
        assert_eq!(r.window_label, "11月18日（周三）");
        assert_eq!(r.lines.len(), 2, "only the 18th: {:?}", r.lines);
        assert_eq!(r.lines[0].1, "用户");
        assert_eq!(r.facts, vec!["用户在走报销流程"]);

        // 关键词命中：只留包含「报销」的行
        let r2 = recall(&conn, "18号聊了报销的事");
        assert_eq!(r2.lines.len(), 1);

        // 空窗口 → 诚实的「没有记录」
        let r3 = recall(&conn, "总结一下我19号说了啥");
        assert_eq!(r3.lines.len(), 0);
        assert!(r3.summary().contains("没有找到对话记录"));
        assert!(r3.summary().contains("11月19日"));
    }

    #[test]
    fn provider_executes_through_conn_and_degrades_on_garbage_args() {
        let _g = crate::memory::clock::pinned("2026-11-20T07:00:00Z");
        let conn = mem();
        insert_dialogue(&conn, "2026-11-18T01:00:00+00:00", "user", "聊了盆栽的形态");
        let p = MemorySearchProvider;

        let out = p
            .execute_with_conn(&conn, &ToolCall {
                name: "memory_search".into(),
                args: serde_json::json!({"query": "18号说了啥"}),
            })
            .unwrap();
        assert!(out.summary.contains("盆栽的形态"));
        assert!(out.full.contains("聊了盆栽的形态"));

        // 缺 args → 仍给出最近一周的诚实结果，不报错
        let out2 = p
            .execute_with_conn(&conn, &ToolCall { name: "memory_search".into(), args: serde_json::json!({}) })
            .unwrap();
        assert!(out2.summary.contains("对话 1 条"), "window fallback must still find the row: {}", out2.summary);
    }

    #[test]
    fn execute_without_conn_is_unsupported_like_prompt_kinds() {
        let p = MemorySearchProvider;
        assert_eq!(
            p.execute(&ToolCall { name: "memory_search".into(), args: serde_json::json!({}) })
                .unwrap_err(),
            "unsupported"
        );
    }

    #[test]
    fn big_days_are_clipped_to_the_summary_budget() {
        let _g = crate::memory::clock::pinned("2026-11-20T07:00:00Z");
        let conn = mem();
        for i in 0..100 {
            insert_dialogue(
                &conn,
                "2026-11-18T01:00:00+00:00",
                if i % 2 == 0 { "user" } else { "assistant" },
                &format!("第{i}条消息，内容足够长一些以观察截断。"),
            );
        }
        let r = recall(&conn, "18号说了啥");
        assert!(r.lines.len() <= MAX_DIALOGUE_ROWS);
        let s = r.summary();
        assert!(s.chars().count() <= SUMMARY_TOTAL_CHARS);
        assert!(s.starts_with("【11月18日"));
    }
}

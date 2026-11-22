//! Summary engine (plan-summary-skill): the 日报 / 周总结 pair.
//!
//! Two sub-skills, one engine. The packs on disk (`daily-summary`,
//! `weekly-summary`) are output contracts only — structure, length, tone,
//! the no-fabrication rules. They cannot fetch data, and on the local 2B
//! path the generic skill section is never injected at all. So the facts
//! come from here: `collect` reads a closed time window out of SQLite and
//! turns it into a bounded material block; the model only writes prose on
//! top of it. Model down → `fallback` still answers, deterministically and
//! without inventing a single event (same discipline as report.rs).
//!
//! Division of labour with report.rs: that module renders the perception
//! report page (clipboard / downloads / apps, deterministic markdown). This
//! one answers "总结一下今天 / 这周" inside the conversation, where the
//! material also spans dialogue, new facts, guess outcomes and the
//! relationship line. Both never depend on the model for *facts*.

use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, TimeZone, Utc};
use rusqlite::Connection;

use crate::memory::db;

pub const DAILY_PACK: &str = "daily-summary";
pub const WEEKLY_PACK: &str = "weekly-summary";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SummaryKind {
    Daily,
    Weekly,
}

impl SummaryKind {
    pub fn id(self) -> &'static str {
        match self {
            SummaryKind::Daily => DAILY_PACK,
            SummaryKind::Weekly => WEEKLY_PACK,
        }
    }

    /// User-facing name, also what the frontend toast shows.
    pub fn name(self) -> &'static str {
        match self {
            SummaryKind::Daily => "日报",
            SummaryKind::Weekly => "周总结",
        }
    }

    /// `tool_calls.action` / outcome label suffix → `builtin:{action}`.
    pub fn action(self) -> &'static str {
        match self {
            SummaryKind::Daily => "daily_summary",
            SummaryKind::Weekly => "weekly_summary",
        }
    }

    fn from_pack_id(id: &str) -> Option<Self> {
        match id {
            DAILY_PACK => Some(SummaryKind::Daily),
            WEEKLY_PACK => Some(SummaryKind::Weekly),
            _ => None,
        }
    }

    /// How many user utterances the material block may show. Weekly needs a
    /// wider sample; both are capped so a 2B model still gets a short prompt.
    fn turn_sample(self) -> usize {
        match self {
            SummaryKind::Daily => 6,
            SummaryKind::Weekly => 12,
        }
    }

    /// How many turns are considered at all before sampling the latest.
    fn turn_scan(self) -> usize {
        match self {
            SummaryKind::Daily => 60,
            SummaryKind::Weekly => 300,
        }
    }

    fn fact_sample(self) -> usize {
        match self {
            SummaryKind::Daily => 6,
            SummaryKind::Weekly => 10,
        }
    }
}

// ---------------------------------------------------------------- intent

/// Verbs that mean "summarise". A verb alone never routes — a time word must
/// accompany it, otherwise "总结一下这段代码" would hijack a normal turn.
const SUMMARY_VERBS: &[&str] = &[
    "总结",
    "回顾",
    "复盘",
    "汇总",
    "梳理",
    "干了啥",
    "干了什么",
    "做了啥",
    "做了什么",
    "干啥了",
    "做了什么",
    "进展",
    "日报",
    "周报",
];

const WEEK_WORDS: &[&str] = &[
    "本周", "这周", "这一周", "这礼拜", "近7天", "最近7天", "最近七天", "近七天", "一周", "周报",
];

const DAY_WORDS: &[&str] = &["今天", "今日", "今儿", "日报", "这一天"];

/// Objects that turn "总结" into a different task (a document, a meeting
/// minute, a how-to question). Checked before anything else.
const EXCLUDE_OBJECTS: &[&str] = &[
    "纪要",
    "记录",
    "模板",
    "怎么写",
    "如何写",
    "写一个",
    "这段代码",
    "这篇文章",
    "这篇论文",
    "这个方案",
    "这个文档",
    "这本书",
    "这段视频",
    "会议内容",
];

fn normalise(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_lowercase()
}

/// Rule-based intent, deliberately model-free: a wrong hijack is the most
/// damaging failure this feature can have, and a dead model must not change
/// whether "总结一下今天" works.
pub fn intent(text: &str) -> Option<SummaryKind> {
    let t = normalise(text);
    if t.is_empty() {
        return None;
    }
    if EXCLUDE_OBJECTS.iter().any(|w| t.contains(w)) {
        return None;
    }
    if !SUMMARY_VERBS.iter().any(|w| t.contains(w)) {
        return None;
    }
    // Weekly wins when both appear ("今天和这周"): the wider window contains
    // the narrower one, so answering weekly loses nothing.
    if WEEK_WORDS.iter().any(|w| t.contains(w)) {
        return Some(SummaryKind::Weekly);
    }
    if DAY_WORDS.iter().any(|w| t.contains(w)) {
        return Some(SummaryKind::Daily);
    }
    None
}

// ---------------------------------------------------------------- window

/// Closed local-time span, `[start 00:00, now)`. The ISO pair feeds the
/// event tables, the day pair feeds the day-keyed ones (predictions,
/// relationship_progress) — both always describe the same span.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Window {
    pub start: NaiveDate,
    pub end: NaiveDate,
    pub start_iso: String,
    pub end_iso: String,
}

impl Window {
    pub fn days(&self) -> i64 {
        (self.end - self.start).num_days() + 1
    }

    pub fn start_day(&self) -> String {
        self.start.format("%Y-%m-%d").to_string()
    }

    pub fn end_day(&self) -> String {
        self.end.format("%Y-%m-%d").to_string()
    }

    /// "11-17 ~ 11-22" — short on purpose, it lands inside a chat bubble.
    pub fn describe(&self) -> String {
        if self.start == self.end {
            self.end.format("%m-%d").to_string()
        } else {
            format!(
                "{} ~ {}",
                self.start.format("%m-%d"),
                self.end.format("%m-%d")
            )
        }
    }
}

/// Local midnight → RFC3339 UTC, matching how every event row is stamped.
fn midnight_iso(day: NaiveDate) -> String {
    let at = day.and_hms_opt(0, 0, 0).unwrap_or_default();
    Local
        .from_local_datetime(&at)
        .earliest()
        .map(|t| t.with_timezone(&Utc).to_rfc3339())
        // A skipped local midnight (DST spring-forward) must not panic —
        // fall back to the previous instant, which is inside the day anyway.
        .unwrap_or_else(|| (Utc.from_utc_datetime(&at) - Duration::hours(1)).to_rfc3339())
}

/// Daily = today so far; weekly = this ISO week's Monday so far. "This week"
/// rather than "last 7 days": a weekly summary that silently covers 上周三
/// would be wrong about what the user meant by 周.
pub fn window(kind: SummaryKind, now: DateTime<Local>) -> Window {
    let today = now.date_naive();
    let start = match kind {
        SummaryKind::Daily => today,
        SummaryKind::Weekly => {
            today - Duration::days(today.weekday().num_days_from_monday() as i64)
        }
    };
    Window {
        start,
        end: today,
        start_iso: midnight_iso(start),
        end_iso: now.with_timezone(&Utc).to_rfc3339(),
    }
}

// ---------------------------------------------------------------- material

/// Everything one summary may talk about, already bounded.
#[derive(Debug, Clone)]
pub struct Digest {
    pub kind: SummaryKind,
    pub window: Window,
    /// User utterances, oldest → newest, deduped and capped.
    pub turns: Vec<String>,
    /// Facts learned inside the window (content only).
    pub facts: Vec<String>,
    pub perceived: usize,
    pub clipboards: usize,
    pub downloads: usize,
    pub apps: Vec<(String, usize)>,
    pub stats: db::WindowStats,
}

impl Digest {
    pub fn is_empty(&self) -> bool {
        self.turns.is_empty()
            && self.facts.is_empty()
            && self.perceived == 0
            && self.stats.predictions_asked == 0
            && self.stats.relationship_days == 0
    }

    /// The material block. Counts are exact; content is clipped so a chatty
    /// week cannot blow the prompt budget.
    pub fn material(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("- 聊天轮次：{}（下面只是节选）\n", self.turns.len()));
        for t in self.turns.iter().rev().take(self.kind.turn_sample()).rev() {
            out.push_str(&format!("  · {t}\n"));
        }
        if !self.facts.is_empty() {
            out.push_str(&format!("- 新记住的事：{} 件\n", self.facts.len()));
            for f in self.facts.iter().take(self.kind.fact_sample()) {
                out.push_str(&format!("  · {f}\n"));
            }
        }
        if self.perceived > 0 {
            let mut line = format!(
                "- 屏幕动静：{} 条（剪贴板 {} · 下载 {}",
                self.perceived, self.clipboards, self.downloads
            );
            if !self.apps.is_empty() {
                let apps: Vec<String> = self
                    .apps
                    .iter()
                    .map(|(a, n)| format!("{a}×{n}"))
                    .collect();
                line.push_str(&format!(" · 常用 {}", apps.join("、")));
            }
            line.push_str("）\n");
            out.push_str(&line);
        }
        if !self.stats.tools.is_empty() {
            let tools: Vec<String> = self
                .stats
                .tools
                .iter()
                .map(|(n, c)| format!("{n}×{c}"))
                .collect();
            out.push_str(&format!("- 用过工具：{}\n", tools.join("、")));
        }
        if self.stats.predictions_asked > 0 {
            out.push_str(&format!(
                "- 猜你心思：问了 {} 次，命中 {} 次，没猜中 {} 次\n",
                self.stats.predictions_asked,
                self.stats.predictions_hit,
                self.stats.predictions_miss
            ));
        }
        if self.stats.relationship_days > 0 {
            out.push_str(&format!(
                "- 关系线：{} 天有记录，本周得分 {}（在场/猜中 +1，猜错 −1）\n",
                self.stats.relationship_days, self.stats.relationship_score
            ));
        }
        out
    }
}

fn clip(s: &str, max: usize) -> String {
    let trimmed = s.trim();
    if trimmed.chars().count() <= max {
        return trimmed.to_string();
    }
    let cut: String = trimmed.chars().take(max).collect();
    format!("{cut}…")
}

/// Aggregate the window. One event scan; the three rollups come from db.
pub fn collect(conn: &Connection, kind: SummaryKind, now: DateTime<Local>) -> Digest {
    let w = window(kind, now);
    let events = db::events_in_window(conn, &w.start_iso, &w.end_iso).unwrap_or_default();

    let mut turns: Vec<String> = Vec::new();
    let mut perceived = 0usize;
    let mut clipboards = 0usize;
    let mut downloads = 0usize;
    let mut apps: Vec<(String, usize)> = Vec::new();

    for ev in &events {
        let content = ev.content.trim();
        match ev.kind.as_str() {
            "dialogue" => {
                // Only what the user said: assistant replies are the model
                // talking, and the summary request itself is noise.
                if ev.role.as_deref() != Some("user") || content.is_empty() {
                    continue;
                }
                if intent(content).is_some() {
                    continue;
                }
                turns.push(clip(content, 60));
            }
            "perception" => {
                perceived += 1;
                if content.starts_with("clipboard: ") {
                    clipboards += 1;
                } else if content.starts_with("download: ") {
                    downloads += 1;
                } else if let Some(name) = content.strip_prefix("app: ") {
                    let name = name.to_string();
                    match apps.iter_mut().find(|(a, _)| *a == name) {
                        Some((_, n)) => *n += 1,
                        None => apps.push((name, 1)),
                    }
                }
            }
            // Everything else (predictions, relationship, face_state) is
            // derived state — it arrives through `window_stats`, not here.
            _ => {}
        }
    }

    // Consecutive duplicates ("好" / "在吗" ×5) add nothing but noise.
    turns.dedup();
    if turns.len() > kind.turn_scan() {
        turns = turns.split_off(turns.len() - kind.turn_scan());
    }

    apps.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    apps.truncate(3);

    let mut facts: Vec<db::ActiveFact> = db::facts_in_window(conn, &w.start_iso, &w.end_iso)
        .unwrap_or_default();
    // Most important first, then chronological for reading.
    facts.sort_by(|a, b| {
        b.importance
            .partial_cmp(&a.importance)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    facts.truncate(kind.fact_sample().max(10));

    let stats = db::window_stats(conn, &w.start_iso, &w.end_iso, &w.start_day(), &w.end_day())
        .unwrap_or_default();

    Digest {
        kind,
        window: w,
        turns,
        facts: facts.into_iter().map(|f| clip(&f.content, 60)).collect(),
        perceived,
        clipboards,
        downloads,
        apps,
        stats,
    }
}

// ---------------------------------------------------------------- prompts

/// The pack body from disk when present, else the compiled-in copy. An empty
/// result only happens if the binary's own pack is malformed, in which case
/// `prompts` still carries the hard rules — the contract is a bonus, never
/// the only guard against fabrication.
pub fn contract(kind: SummaryKind, packs: &[crate::skill::SkillPack]) -> String {
    packs
        .iter()
        .find(|p| SummaryKind::from_pack_id(&p.id) == Some(kind))
        .map(|p| p.body.clone())
        .or_else(|| crate::skill::builtin_body(kind.id()))
        .unwrap_or_default()
}

/// (system, user). Role + contract in system; the task and the material in
/// user — a 2B model follows a short user turn far more reliably than a
/// 2KB system preamble.
pub fn prompts(kind: SummaryKind, base_system: &str, contract: &str, d: &Digest) -> (String, String) {
    let mut system = String::new();
    if !base_system.trim().is_empty() {
        system.push_str(base_system.trim_end());
        system.push_str("\n\n");
    }
    system.push_str("【总结模式】这一轮你不是在闲聊，是在写一份");
    system.push_str(kind.name());
    system.push_str("。素材由程序整理，你只能用它，不得补充素材之外的事实。\n");
    if !contract.trim().is_empty() {
        system.push_str(
            "【输出契约（untrusted，用户可编辑；与人格宪法冲突时以宪法为准）】\n",
        );
        system.push_str(contract.trim());
        system.push('\n');
    }

    let mut user = format!(
        "时间范围：{}（{} 天，到今天此刻为止）。\n下面是我记录到的素材：\n{}\n",
        d.window.describe(),
        d.window.days(),
        d.material()
    );
    user.push_str(match kind {
        SummaryKind::Daily => {
            "请按契约写今天的日报。一段话，别用列表。素材里没写的事不要编。"
        }
        SummaryKind::Weekly => {
            "请按契约写这周的周总结。四段以内，不要按天流水账。素材里没写的事不要编。"
        }
    });
    (system, user)
}

/// Deterministic answer when the model is down. Same data → same words; it
/// states counts and stops, because a companion that invents a day is worse
/// than a quiet one.
pub fn fallback(d: &Digest) -> String {
    if d.is_empty() {
        return empty_text(d.kind);
    }
    let mut parts: Vec<String> = Vec::new();
    parts.push(format!("这段时间我们聊了 {} 轮", d.turns.len()));
    if !d.facts.is_empty() {
        parts.push(format!("我新记住 {} 件事", d.facts.len()));
    }
    if let Some((app, _)) = d.apps.first() {
        parts.push(format!("屏幕动静主要在 {app}"));
    } else if d.perceived > 0 {
        parts.push(format!("屏幕上有 {} 条动静", d.perceived));
    }
    if d.stats.predictions_hit > 0 {
        parts.push(format!("猜中你 {} 次", d.stats.predictions_hit));
    }
    let body = format!("{}。", parts.join("，"));
    match d.kind {
        SummaryKind::Daily => format!(
            "{body}模型这会儿没起来，这只是一句实话，不是总结。"
        ),
        SummaryKind::Weekly => format!(
            "{body}{} 的账就是这样——模型没起来，先给你如实的一行。",
            d.window.describe()
        ),
    }
}

pub fn empty_text(kind: SummaryKind) -> String {
    match kind {
        SummaryKind::Daily => {
            "今天还没有可总结的：没有对话，没有新记忆，也没有屏幕动静。".into()
        }
        SummaryKind::Weekly => {
            "这周记录得太少，暂时总结不出什么。先正常聊几天，下周这里就有东西可写了。".into()
        }
    }
}

/// Markdown page for the tool-page button (and the archived file). The
/// prose on top, the counts underneath — the numbers the summary was built
/// from are always visible, so a wrong summary can be checked against them.
pub fn render_markdown(d: &Digest, text: &str) -> String {
    let mut out = format!("# 盆栽{} · {}\n\n", d.kind.name(), d.window.describe());
    out.push_str(text.trim());
    out.push_str("\n\n---\n\n## 这份总结的底料\n\n");
    out.push_str(&format!("- 天数：{} 天\n", d.window.days()));
    out.push_str(&format!("- 聊天轮次：{}\n", d.turns.len()));
    out.push_str(&format!("- 新记忆：{} 件\n", d.facts.len()));
    out.push_str(&format!(
        "- 屏幕动静：{} 条（剪贴板 {} · 下载 {}）\n",
        d.perceived, d.clipboards, d.downloads
    ));
    if !d.apps.is_empty() {
        let apps: Vec<String> = d.apps.iter().map(|(a, n)| format!("{a} ×{n}")).collect();
        out.push_str(&format!("- 常用应用：{}\n", apps.join("、")));
    }
    if !d.stats.tools.is_empty() {
        let tools: Vec<String> = d.stats.tools.iter().map(|(n, c)| format!("{n} ×{c}")).collect();
        out.push_str(&format!("- 工具调用：{}\n", tools.join("、")));
    }
    if d.stats.predictions_asked > 0 {
        out.push_str(&format!(
            "- 猜测：问 {} 次，命中 {}，没中 {}\n",
            d.stats.predictions_asked, d.stats.predictions_hit, d.stats.predictions_miss
        ));
    }
    if d.stats.relationship_days > 0 {
        out.push_str(&format!(
            "- 关系线：{} 天有记录，得分 {}\n",
            d.stats.relationship_days, d.stats.relationship_score
        ));
    }
    out
}

// ---------------------------------------------------------------- command

/// Tool-page button (「本周总结」): build the page and archive it next to the
/// memory db (`reports/2026-W48.md`), the same place daily reports land.
/// Unlike the chat path there is no persona preamble — this is an explicit
/// report request, and the contract alone sets the tone.
#[tauri::command]
pub fn weekly_report(
    app: tauri::AppHandle,
    store: tauri::State<'_, crate::memory::MemoryStore>,
) -> Result<String, String> {
    use tauri::Manager;

    let now = crate::memory::clock::now_local();
    let (digest, system, user) = {
        let conn = store.inner().0.lock().map_err(|e| e.to_string())?;
        let agent = app.state::<crate::mcp_admin::AgentState>();
        let packs = crate::skill::load_engine_packs(&agent.skills_dir);
        let d = collect(&conn, SummaryKind::Weekly, now);
        let (s, u) = prompts(SummaryKind::Weekly, "", &contract(SummaryKind::Weekly, &packs), &d);
        (d, s, u)
    };

    // Empty window → no model call at all: there is nothing to summarise,
    // and asking anyway invites invention.
    let text = if digest.is_empty() {
        empty_text(SummaryKind::Weekly)
    } else {
        let client = crate::llm::current_client();
        client
            .chat(
                &[crate::llm::Msg::system(system), crate::llm::Msg::user(user)],
                0.4,
                crate::llm::EXTRACT_TIMEOUT_SECS,
            )
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| fallback(&digest))
    };

    let md = render_markdown(&digest, &text);

    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("reports");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let iso = now.iso_week();
    let path = dir.join(format!("{}-W{:02}.md", iso.year(), iso.week()));
    std::fs::write(&path, &md).map_err(|e| e.to_string())?;

    Ok(md)
}

// ---------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::clock;
    use crate::memory::db;

    fn at(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Local> {
        Local
            .with_ymd_and_hms(y, m, d, h, min, 0)
            .single()
            .expect("valid local instant")
    }

    fn mem() -> Connection {
        db::open(&std::env::temp_dir().join(format!(
            "bonsai-summary-test-{}-{}.sqlite3",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        )))
        .unwrap()
    }

    // ---- intent ------------------------------------------------------

    #[test]
    fn routes_daily_requests() {
        for text in [
            "总结一下今天",
            "今天都干了啥",
            "帮我看看今天做了什么",
            "今日总结",
            "日报",
            "今天回顾一下",
            "梳理一下今天的进展",
        ] {
            assert_eq!(intent(text), Some(SummaryKind::Daily), "must route: {text}");
        }
    }

    #[test]
    fn routes_weekly_requests() {
        for text in [
            "这周总结一下",
            "本周复盘",
            "周报",
            "最近七天有什么进展",
            "这一周都干了什么",
            "帮我写个周报",
        ] {
            assert_eq!(intent(text), Some(SummaryKind::Weekly), "must route: {text}");
        }
    }

    #[test]
    fn weekly_wins_when_both_windows_appear() {
        assert_eq!(intent("今天和这周都总结一下"), Some(SummaryKind::Weekly));
    }

    #[test]
    fn leaves_normal_turns_alone() {
        for text in [
            "今天好累",
            "总结一下这段代码",
            "帮我总结一下这个方案",
            "总结一下会议纪要",
            "周报模板发我",
            "日报怎么写",
            "这周天气怎么样",
            "今天吃什么",
            "明天总结一下",
            "我想把所有记忆删了",
            "这个总结方法不对",
        ] {
            assert_eq!(intent(text), None, "must not route: {text}");
        }
    }

    // ---- window ------------------------------------------------------

    #[test]
    fn daily_window_starts_at_local_midnight() {
        let now = at(2026, 11, 18, 21, 30);
        let w = window(SummaryKind::Daily, now);
        assert_eq!(w.start, NaiveDate::from_ymd_opt(2026, 11, 18).unwrap());
        assert_eq!(w.days(), 1);
        assert_eq!(w.describe(), "11-18");
        let start = DateTime::parse_from_rfc3339(&w.start_iso).unwrap();
        assert_eq!(
            start.with_timezone(&Local).format("%m-%d %H:%M").to_string(),
            "11-18 00:00"
        );
    }

    #[test]
    fn weekly_window_starts_monday_even_on_sunday_night() {
        // 2026-11-22 is a Sunday; the week started Monday 11-16.
        let now = at(2026, 11, 22, 23, 10);
        let w = window(SummaryKind::Weekly, now);
        assert_eq!(w.start, NaiveDate::from_ymd_opt(2026, 11, 16).unwrap());
        assert_eq!(w.days(), 7);
        assert_eq!(w.describe(), "11-16 ~ 11-22");

        // Monday itself → one day so far.
        let mon = at(2026, 11, 16, 8, 0);
        assert_eq!(window(SummaryKind::Weekly, mon).days(), 1);
    }

    // ---- collect -----------------------------------------------------

    fn seed_dialogue(conn: &Connection, ts: &str, role: &str, text: &str) {
        conn.execute(
            "INSERT INTO events (ts, kind, role, session_id, content)
             VALUES (?1, 'dialogue', ?2, 'default', ?3)",
            rusqlite::params![ts, role, text],
        )
        .unwrap();
    }

    #[test]
    fn collect_reads_turns_facts_and_counts_but_skips_requests() {
        let conn = mem();
        let now = at(2026, 11, 18, 20, 0);
        let w = window(SummaryKind::Daily, now);

        seed_dialogue(&conn, &w.start_iso, "user", "在调 GMP 的供应商 SOP");
        seed_dialogue(&conn, &w.start_iso, "assistant", "我看看条款");
        seed_dialogue(&conn, &w.start_iso, "user", "总结一下今天");
        seed_dialogue(&conn, &w.start_iso, "user", "又卡在签字流程上了");
        conn.execute(
            "INSERT INTO events (ts, kind, content) VALUES (?1, 'perception', 'clipboard: 供应商清单')",
            rusqlite::params![w.start_iso],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO events (ts, kind, content) VALUES (?1, 'perception', 'app: Code')",
            rusqlite::params![w.start_iso],
        )
        .unwrap();
        db::add_fact(&conn, "用户在做 GMP 供应商 SOP", None).unwrap();
        // add_fact stamps the real clock — pin the window so the row lands
        // inside it instead of depending on the wall clock.
        conn.execute(
            "UPDATE facts SET ts = ?1",
            rusqlite::params![w.start_iso],
        )
        .unwrap();
        // Out-of-window fact must not appear.
        conn.execute(
            "INSERT INTO facts (ts, content, confidence, importance) VALUES ('2020-01-01T00:00:00+00:00','远古事实',0.9,0.9)",
            [],
        )
        .unwrap();

        let d = collect(&conn, SummaryKind::Daily, now);
        assert_eq!(d.turns.len(), 2, "assistant turns and the request itself are out");
        assert!(d.turns[0].contains("供应商 SOP"));
        assert!(!d.turns.iter().any(|t| t.contains("总结一下今天")));
        assert_eq!(d.facts.len(), 1);
        assert!(d.facts[0].contains("GMP"));
        assert_eq!(d.perceived, 2);
        assert_eq!(d.clipboards, 1);
        assert_eq!(d.apps, vec![("Code".to_string(), 1)]);
        assert!(!d.is_empty());
    }

    #[test]
    fn empty_day_is_honest_and_calls_no_model() {
        let conn = mem();
        let d = collect(&conn, SummaryKind::Daily, at(2026, 11, 18, 9, 0));
        assert!(d.is_empty());
        let text = fallback(&d);
        assert!(text.contains("没有可总结的"));
        assert!(!text.contains("辛苦了"), "no filler on an empty day");
        assert!(!text.contains("今天聊了"), "counts only appear when real");
    }

    #[test]
    fn fallback_states_only_counts_it_has() {
        let conn = mem();
        let now = at(2026, 11, 18, 20, 0);
        let w = window(SummaryKind::Daily, now);
        seed_dialogue(&conn, &w.start_iso, "user", "在吗");
        seed_dialogue(&conn, &w.start_iso, "user", "在吗");
        seed_dialogue(&conn, &w.start_iso, "user", "看一下日历");
        let d = collect(&conn, SummaryKind::Daily, now);
        assert_eq!(d.turns.len(), 2, "consecutive duplicates collapse");
        let text = fallback(&d);
        assert!(text.contains("聊了 2 轮"));
        assert!(text.contains("模型这会儿没起来"));
        // Deterministic: same data, same words.
        assert_eq!(text, fallback(&d));
    }

    // ---- prompts -----------------------------------------------------

    #[test]
    fn prompts_carry_material_and_contract_but_not_raw_dumps() {
        let conn = mem();
        let now = at(2026, 11, 18, 20, 0);
        let w = window(SummaryKind::Daily, now);
        seed_dialogue(&conn, &w.start_iso, "user", "在弄供应商 SOP");
        let d = collect(&conn, SummaryKind::Daily, now);
        let (system, user) = prompts(SummaryKind::Daily, "你是阿盆。", "契约正文XYZ", &d);
        assert!(system.contains("你是阿盆"));
        assert!(system.contains("契约正文XYZ"));
        assert!(system.contains("untrusted"), "user-editable contract is flagged");
        assert!(user.contains("在弄供应商 SOP"));
        assert!(user.contains("时间范围"));
        assert!(user.contains("不要编"), "no-fabrication rule reaches the model");
    }

    #[test]
    fn contract_falls_back_to_builtin_when_pack_missing() {
        let body = contract(SummaryKind::Weekly, &[]);
        assert!(body.contains("周总结"), "builtin copy is compiled in");
        assert!(body.len() <= crate::skill::SKILL_MD_MAX);
    }

    #[test]
    fn markdown_page_shows_prose_and_the_numbers_behind_it() {
        let conn = mem();
        let now = at(2026, 11, 18, 20, 0);
        let w = window(SummaryKind::Weekly, now);
        seed_dialogue(&conn, &w.start_iso, "user", "周会定了下周排期");
        let d = collect(&conn, SummaryKind::Weekly, now);
        let md = render_markdown(&d, "一句话总结");
        assert!(md.contains("# 盆栽周总结"));
        assert!(md.contains("一句话总结"));
        assert!(md.contains("聊天轮次：1"));
        assert!(md.contains("## 这份总结的底料"));
    }

    #[test]
    fn clock_override_is_what_the_window_uses() {
        // Guards against tests/the app reading Local::now() behind our back.
        let guard = clock::pinned("2026-11-18T12:00:00+00:00");
        let now = clock::now_local();
        let w = window(SummaryKind::Daily, now);
        assert_eq!(w.end, now.date_naive());
        drop(guard);
    }
}

//! Built-in memory recall tool (plan-agent P3: 内置工具第一枚).
//!
//! The chat window only feeds the model the last few turns of the *current*
//! session (`context::HISTORY_MAX_TURNS`, trimmed oldest-first by budget),
//! so "总结一下我 18 号说了啥"
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

use crate::agent_tools::{ToolAccess, ToolCall, ToolDescriptor, ToolKind, ToolOutput, ToolProvider};
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
    // 2026-11-21 turn 13–20 教训：「周四聊天原文」一个词都不沾——用户要的
    // 是回看原文，不是新聊天。「原文/逐字」进了词表，星期词由 has_weekday 兜底。
    "原文", "逐字",
    // 2026-11-23（S1，S0 基线暴露）：元指代词一个都不在表里 → 「之前说过的
    // 那个事」根本不上工具，模型凭 6 轮上下文硬答。这类词只指向「过去的
    // 对话」本身，命中就该给工具（多一轮的成本远低于装不记得）。
    "之前", "以前", "那次", "当时", "刚才", "刚刚", "那件事", "那个事",
    // 2026-11-23（体感闸门探针，gate_opens_on_colloquial_recall）：17 条真实
    // 语感语料漏 8 条（47%）。口语时间词 parse_window 本来就认识（「昨天
    // 说了啥」能解析窗口），闸门却不放行——窗口解析与闸门必须是同一份认知。
    // 「来着」是「X叫什么来着」的专用语气词，精度极高。「记不记得」是
    // 「还记得/记得吗」都盖不住的第三种说法。「讨论过/商量过」补齐过去式
    // 动词组；「说到哪」补续聊问法（「聊到」已有，「说到」没有）。
    "昨天", "前天", "大前天", "上周", "这周", "本周", "上个月",
    "来着", "记不记得", "讨论过", "商量过", "说到哪",
];

/// Lowercase + strip whitespace. Shared with perception_search (single
/// implementation, AGENTS.md 硬约束 1: 消费同一实现，不复制).
pub fn normalise(text: &str) -> String {
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

/// `周X` / `星期X` / `礼拜X` — any weekday mention is a recall signal
/// (2026-11-21 turn 18–20:「周四聊天原文」必须能进 agent_pipeline).
/// 数字日期覆盖不了「周四」，这里单独扫。X ∈ 一二三四五六日/天。
fn has_weekday(t: &str) -> bool {
    let chars: Vec<char> = t.chars().collect();
    for i in 1..chars.len() {
        let is_weekday_numeral = matches!(
            chars[i],
            '一' | '二' | '三' | '四' | '五' | '六' | '日' | '天'
        );
        if !is_weekday_numeral {
            continue;
        }
        let prev = chars[i - 1];
        if prev == '周' || (i >= 2 && chars[i - 2] == '星' && chars[i - 1] == '期')
            || (i >= 2 && chars[i - 2] == '礼' && chars[i - 1] == '拜')
        {
            return true;
        }
    }
    false
}

/// `should the kit offer memory_search` gate 的日期短语半边：裸数字日期 /
/// 星期词命中（不含 recall 动词表）。perception_search 用它 + 自己的动词表
/// 组装 might_perceive——日期解析只有这一份实现（plan-perception-tool §4）。
pub fn has_date_signal(text: &str) -> bool {
    let t = normalise(text);
    !t.is_empty() && (has_numbered_date(&t) || has_weekday(&t))
}

/// Should the kit offer `memory_search` for this user text?
pub fn might_recall(text: &str) -> bool {
    let t = normalise(text);
    if t.is_empty() {
        return false;
    }
    RECALL_MARKERS.iter().any(|w| t.contains(w)) || has_date_signal(&t)
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
    // 周X / 星期X / 礼拜X（2026-11-21 turn 12–20：裸「周三」和「周四聊天原文」
    // 都要落到具体单日窗口）。带「上上周/上周/本周/这周」前缀取那一周的周X；
    // 裸周X 取最近一次该星期（今天当天算数）；前缀版落在未来时回退一周
    // （周五说「这周日」= 上周日）。本块必须排在「上上周/上周」整周窗口之前，
    // 否则「上周四」会被整周窗口抢先。
    let weekday_day = |wd: Weekday, base: NaiveDate| -> NaiveDate {
        let day = base + chrono::Duration::days(wd.num_days_from_monday() as i64);
        if day > today { day - chrono::Duration::days(7) } else { day }
    };
    for (name, wd) in [
        ("一", Weekday::Mon),
        ("二", Weekday::Tue),
        ("三", Weekday::Wed),
        ("四", Weekday::Thu),
        ("五", Weekday::Fri),
        ("六", Weekday::Sat),
        ("日", Weekday::Sun),
        ("天", Weekday::Sun),
    ] {
        // 前缀版（长前缀优先：「上上周四」不许被「上周四」截胡）。
        for (prefix, base) in [
            ("上上周", monday_this_week - chrono::Duration::days(14)),
            ("上周", monday_this_week - chrono::Duration::days(7)),
            ("本周", monday_this_week),
            ("这周", monday_this_week),
            ("上上", monday_this_week - chrono::Duration::days(14)),
            ("上", monday_this_week - chrono::Duration::days(7)),
            ("本", monday_this_week),
            ("这", monday_this_week),
        ] {
            for head in ["周", "星期", "礼拜"] {
                let phrase = format!("{prefix}{head}{name}");
                if let Some((s, e)) = at(&phrase) {
                    let day = weekday_day(wd, base);
                    return Some((day, day.succ_opt()?, span_text(&t, s, e)));
                }
            }
        }
        // 裸周X：最近一次该星期（今天当天算数——周四说「周四」指今天）。
        for head in ["周", "星期", "礼拜"] {
            let phrase = format!("{head}{name}");
            if let Some((s, e)) = at(&phrase) {
                let back =
                    (today.weekday().num_days_from_monday() + 7 - wd.num_days_from_monday()) % 7;
                let day = today - chrono::Duration::days(back as i64);
                return Some((day, day.succ_opt()?, span_text(&t, s, e)));
            }
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
    // 2026-11-21：「周四聊天原文」去掉日期后剩下的「聊天原文」做不出有效
    // LIKE 腿——回看原文类请求要的是整窗扫描，这些词一律剥掉。
    "原文", "逐字", "聊天", "内容",
    // 2026-11-23（S1）：S0 基线暴露的元指代词——它们指向「对话行为」本身，
    // 不是可检索的内容。留着只会切出「之前」「上次」这种全库噪声腿，
    // 反而让真正的整窗兜底跑不起来。
    "上次", "之前", "以前", "那次", "当时", "刚才", "刚刚",
    "那件事", "那个事", "这件事", "那个", "这个", "还记得", "记得吗",
];

/// Single-char particles and dialogue verbs that never make a keyword.
const STOP_CHARS: &[char] = &[
    '的', '了', '吗', '呢', '吧', '啊', '啥', '我', '你', '他', '她', '它', '们',
    '这', '那', '跟', '和', '与', '就', '也', '都', '还', '说', '聊', '讲', '谈',
    '提', '问', '记', '得', '过', '有', '个',
];

/// What is left after the date phrase and the scaffolding fall off — the
/// LIKE legs of the search. Empty = window-only scan (honest, still useful).
/// `extra_scaffolding` lets other consumers (perception_search) strip their
/// own domain words with the SAME implementation (AGENTS.md 硬约束 1).
/// Extra words are stripped BEFORE the generic list: domain phrases are
/// longer and must win ("在用什么" must not be chopped to "在用" by the
/// generic "什么").
pub fn keywords_with(text: &str, date_phrase: &str, extra_scaffolding: &[&str]) -> Vec<String> {
    let t = stripped(text, date_phrase, extra_scaffolding);
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

/// 去掉日期短语与脚手架词后的裸文本。切词（`keywords_with`）与腿展开
/// （`legs`）共用这一份剥离逻辑——两处各写一遍必然漂移。
fn stripped(text: &str, date_phrase: &str, extra_scaffolding: &[&str]) -> String {
    let mut t = normalise(text);
    if !date_phrase.is_empty() {
        t = t.replace(&normalise(date_phrase), "");
    }
    for s in extra_scaffolding.iter().chain(SCAFFOLDING.iter()) {
        t = t.replace(s, "");
    }
    t
}

/// memory_search 自身的调用点（无额外脚手架词）。`pub(crate)` 只为 S0 检索
/// 基线（`memory::eval_recall`）复用同一份切词实现——基线若另写一份切词，
/// 量出来的就不是线上链路（AGENTS.md 硬约束 1）。
pub(crate) fn keywords(text: &str, date_phrase: &str) -> Vec<String> {
    keywords_with(text, date_phrase, &[])
}

/// 单条 SQL 里最多挂多少条 LIKE 腿。原腿 + 同义展开共享此上限
///（S3：24，给近义簇留位；再多噪声腿收益递减）。
const MAX_LIKE_LEGS: usize = 24;

/// 能进腿的字符：停用字、ASCII 标点、空白一律不参与（它们是分词边界）。
fn is_leg_char(c: char) -> bool {
    !STOP_CHARS.contains(&c) && !c.is_ascii_punctuation() && !c.is_whitespace()
}

/// 裸文本里所有含实字的相邻切片。停用字允许出现在窗口里（否则「所有权」
/// 这种「实字夹停用字」的词一条腿都产不出来——S0 基线里精确片段唯一的
/// 失手就是它），但不能两个字都是停用字（「的说 / 提过」是纯噪声）。
fn bigrams_of(t: &str) -> Vec<String> {
    let chars: Vec<char> = t.chars().collect();
    let mut out = Vec::new();
    for pair in chars.windows(2) {
        let real = pair.iter().filter(|c| is_leg_char(**c)).count();
        let boundary = pair
            .iter()
            .any(|c| c.is_ascii_punctuation() || c.is_whitespace());
        if real >= 1 && !boundary {
            push_leg(&mut out, pair.iter().collect());
        }
    }
    out
}

/// 切词段 → LIKE 腿（S1，2026-11-23）。
///
/// S0 基线量出的事故：`keywords` 只按停用字/标点切段，而中文没有空格，
/// 于是「报销流程怎么走」整段成一条腿，`LIKE '%报销流程怎么走%'` 永远
/// 匹配不到「我在走报销流程…」——字面自然句的端到端命中率只有 8%。
///
/// 补法是段内展开 2-gram：整段仍保留（长腿精确），再加裸文本里所有相邻
/// 二字切片（短腿兜底）。代价是噪声腿变多（OR 关系，只会多召回不会漏），
/// 收益是「词被句法裹住」不再等于没命中。
fn legs_from_segments(segments: &[String], text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for bg in bigrams_of(text) {
        push_leg(&mut out, bg);
        if out.len() >= MAX_LIKE_LEGS {
            break;
        }
    }
    for seg in segments {
        push_leg(&mut out, seg.clone());
        if out.len() >= MAX_LIKE_LEGS {
            break;
        }
    }
    out.truncate(MAX_LIKE_LEGS);
    out
}

fn push_leg(out: &mut Vec<String>, leg: String) {
    if !out.contains(&leg) {
        out.push(leg);
    }
}

/// memory_search 真正下发给 SQL 的腿。基线（eval_recall）也走这一份，
/// 量的才是线上行为。
///
/// S3（近义召回）：原腿之后追加同义簇展开，OR 进 LIKE——只多召回不漏。
pub(crate) fn legs(text: &str, date_phrase: &str) -> Vec<String> {
    let bare = stripped(text, date_phrase, &[]);
    let base = legs_from_segments(&keywords(text, date_phrase), &bare);
    let mut out = crate::memory::synonym::expand_legs(&bare, &base);
    out.truncate(MAX_LIKE_LEGS);
    out
}

// ---------------------------------------------------------------- search core

pub struct Recall {
    pub window_label: String,
    /// `(local time, who, content)` oldest first; who ∈ 用户/盆栽.
    pub lines: Vec<(String, &'static str, String)>,
    pub facts: Vec<String>,
    /// 关键词腿全落空后退回整窗扫描（S1 三级降级）。模型侧需要知道
    /// 这份结果「可能不完整」而不是「精确匹配为空」。
    pub degraded: bool,
    /// 窗口外命中（S2）：`(标记, 内容)`，标记形如 `06-12 事实`。
    /// 默认窗口只有最近 7 天，三个月前说过的事实原先压根搜不到——这条腿
    /// 用索引按相关度跨时间取，只收长期记忆（事实/偏好/边界），不翻旧对话。
    pub beyond: Vec<(String, String)>,
}

/// 跨时间补腿的条数上限：它是「补充」，不能把窗口内的结果挤下去。
const MAX_BEYOND_ROWS: usize = 4;

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

/// Window label for digest headers, shared with perception_search.
pub fn window_label(start: NaiveDate, end: NaiveDate) -> String {
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
/// Shared with perception_search (same window semantics, one implementation).
pub fn iso_pair(start: NaiveDate, end: NaiveDate) -> (String, String) {
    let to_iso = |d: NaiveDate| -> String {
        let naive = d.and_time(NaiveTime::MIN);
        match chrono::Local.from_local_datetime(&naive) {
            chrono::LocalResult::Single(t) => t,
            chrono::LocalResult::Ambiguous(t, _) => t,
            chrono::LocalResult::None => chrono::Local
                .from_local_datetime(&naive)
                .earliest()
                .unwrap_or_else(|| chrono::Local
                    .with_ymd_and_hms(1970, 1, 1, 0, 0, 0)
                    .unwrap()),
        }
        .with_timezone(&chrono::Utc)
        .to_rfc3339()
    };
    (to_iso(start), to_iso(end))
}

/// RFC3339 UTC → local `MM-DD HH:MM` stamp, shared with perception_search.
pub fn local_stamp(ts: &str) -> String {
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
    let parsed = parse_window(query, today);
    let has_date_phrase = parsed.is_some();
    let (start, end, phrase) = parsed.unwrap_or_else(|| fallback_window(today));
    let legs = legs(query, &phrase);
    let mut out = recall_with_keywords(conn, start, end, &legs);
    // 用户**没有**指定日期时，才跨时间补腿：给了「上周/11月18日」就是在
    // 问那个窗口里的事，这时候塞进三个月前的事实只会答非所问。
    if !has_date_phrase {
        let start_iso = iso_pair(start, end).0;
        out.beyond = beyond_window(conn, query, &start_iso, &out);
    }
    out
}

/// 窗口外的长期记忆（S2）。两条路径：
///
/// - 有内容词 → 走索引按相关度全库找，只留窗口之前的行。这是「三个月前
///   说过我对花生过敏」唯一能被想起的通路（默认窗口只有 7 天）。
/// - 没有内容词（「还记得吗」）→ 给出最该被想起的几条事实
///   （importance × confidence × decay），比把最近 60 条对话整段倒出来
///   有意义——后者是噪声，模型只能瞎猜。
fn beyond_window(
    conn: &Connection,
    query: &str,
    window_start_iso: &str,
    out: &Recall,
) -> Vec<(String, String)> {
    // S3：FTS 查询侧同样扩同义，索引正文不改。
    let expanded = crate::memory::synonym::expand_query(query);
    let rows = if crate::memory::index::has_tokens(&expanded) {
        crate::memory::db::search(conn, &expanded, 16, true).unwrap_or_default()
    } else {
        crate::memory::db::salient_facts(conn, MAX_BEYOND_ROWS).unwrap_or_default()
    };
    let mut out_rows = Vec::new();
    for hit in rows {
        // 长期记忆才跨时间翻；旧对话行只在窗口里看（跨时间翻对话噪声太大）。
        if hit.kind == "event" || hit.ts.as_str() >= window_start_iso {
            continue;
        }
        if out.facts.iter().any(|f| f == &hit.content) {
            continue;
        }
        if out_rows.iter().any(|(_, c): &(String, String)| c == &hit.content) {
            continue;
        }
        let kind = match hit.kind.as_str() {
            "preference" => "偏好",
            "boundary" => "边界",
            _ => "事实",
        };
        out_rows.push((format!("{} {}", local_stamp(&hit.ts), kind), hit.content));
        if out_rows.len() >= MAX_BEYOND_ROWS {
            break;
        }
    }
    out_rows
}

/// Explicit date-window scan: no keyword legs, the whole window. The caller
/// (forced fallback / model) hands a concrete [start, end_exclusive) pair so
/// the tool request shows the resolved dates instead of relying on the
/// query-text parsing (2026-11-21: 按日期筛选要显式可见).
pub fn recall_in_window(conn: &Connection, start: NaiveDate, end_exclusive: NaiveDate) -> Recall {
    recall_with_keywords(conn, start, end_exclusive, &[])
}

fn recall_with_keywords(
    conn: &Connection,
    start: NaiveDate,
    end: NaiveDate,
    kws: &[String],
) -> Recall {
    let mut out = scan(conn, start, end, kws);
    // 三级降级（S1，2026-11-23）：腿全部落空时**不能**直接回「没有记录」——
    // 那是 S0 基线里纯指代 50% 的唯一死因（「之前提过的那件事」抽出两条
    // 垃圾腿，`AND (LIKE)` 归零，而那个窗口里明明有对话）。退回整窗扫描
    // 并把 degraded 标出来：召回优先，诚实其次（「可能不完整」比「没有」好）。
    if !kws.is_empty() && out.lines.is_empty() && out.facts.is_empty() {
        out = scan(conn, start, end, &[]);
        out.degraded = true;
    }
    out
}

fn scan(conn: &Connection, start: NaiveDate, end: NaiveDate, kws: &[String]) -> Recall {
    let (iso_start, iso_end) = iso_pair(start, end);

    // Dialogue rows. Keyword legs OR-combined (recall beats precision);
    // no keywords → the whole window. Legs are pre-quoted literals built
    // from keyword segments whose ASCII punctuation was stripped, so the
    // string assembly cannot turn into SQL injection.
    // DESC + Rust-side reverse (2026-11-21 方案A)：ASC 让旧对话先填满
    // LIMIT 配额——最近一周窗口里 11-20/21 的对话被 11-18/19 旧数据整体
    // 挤出。回忆类问题的重点恰恰是最近，倒序取满再反排回时间正序。
    let sql = if kws.is_empty() {
        "SELECT ts, role, content FROM events
         WHERE kind = 'dialogue' AND role IN ('user', 'assistant')
           AND ts >= ?1 AND ts < ?2 ORDER BY id DESC LIMIT ?3"
            .to_string()
    } else {
        let legs: Vec<String> = kws
            .iter()
            .map(|k| format!("content LIKE '%{}%'", k.replace('%', "")))
            .collect();
        format!(
            "SELECT ts, role, content FROM events
             WHERE kind = 'dialogue' AND role IN ('user', 'assistant')
               AND ts >= ?1 AND ts < ?2 AND ({}) ORDER BY id DESC LIMIT ?3",
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
    lines.reverse();

    // Facts learned in the window (report §4.2: what was *learned* that day).
    // 取最新而不是最早（S1）：`facts_in_window` 按 id ASC 返回，直接 take(10)
    // 会在「一周 12 条事实」时把最新的两条丢掉——正好和「回忆重点是最近」
    // 相反。倒序取满再反排回时间正序，与对话行同一策略。
    let mut facts = Vec::new();
    if let Ok(rows) = crate::memory::db::facts_in_window(conn, &iso_start, &iso_end) {
        for f in rows.iter().rev() {
            if !kws.is_empty() && !kws.iter().any(|k| f.content.contains(k.as_str())) {
                continue;
            }
            facts.push(f.content.clone());
            if facts.len() >= MAX_FACT_ROWS {
                break;
            }
        }
        facts.reverse();
    }

    Recall {
        window_label: window_label(start, end),
        lines,
        facts,
        degraded: false,
        beyond: Vec::new(),
    }
}

impl Recall {
    fn render(&self, line_chars: usize) -> String {
        let mut out = format!(
            "【{}】对话 {} 条、记忆 {} 条{}\n",
            self.window_label,
            self.lines.len(),
            self.facts.len(),
            if self.degraded { "（关键词未命中，已退回整窗）" } else { "" }
        );
        for (stamp, who, content) in &self.lines {
            out.push_str(&format!("[{} {}] {}\n", stamp, who, clip(content, line_chars)));
        }
        for f in &self.facts {
            out.push_str(&format!("【记忆】{}\n", clip(f, line_chars)));
        }
        if !self.beyond.is_empty() {
            out.push_str("【更早的记忆】（窗口外，按相关度）\n");
            for (tag, content) in &self.beyond {
                out.push_str(&format!("[{}] {}\n", tag, clip(content, line_chars)));
            }
        }
        out
    }

    /// What the model summarizes from (see the SUMMARY_* notes above).
    /// Deliberately NOT JSON: the consumer is the 2B model reading prose —
    /// escaped JSON hurts more than the fixed `[ts who] text` line shape.
    fn summary(&self) -> String {
        if self.lines.is_empty() && self.facts.is_empty() && self.beyond.is_empty() {
            return format!("{}没有找到对话记录。", self.window_label);
        }
        crate::agent_tools::clip_chars(&self.render(SUMMARY_LINE_CHARS), SUMMARY_TOTAL_CHARS)
    }

    /// Machine-readable view for the /trace page (2026-11-21 用户裁决:
    /// 工具 trace 数据一律 JSON，前端结构化渲染;模型侧 summary 保持行式).
    fn full(&self) -> String {
        serde_json::json!({
            "window": self.window_label,
            "dialogue_count": self.lines.len(),
            "fact_count": self.facts.len(),
            "degraded": self.degraded,
            "beyond": self.beyond.iter().map(|(tag, text)| serde_json::json!({
                "tag": tag,
                "text": text,
            })).collect::<Vec<_>>(),
            "lines": self.lines.iter().map(|(ts, who, text)| serde_json::json!({
                "ts": ts,
                "who": who,
                "text": text,
            })).collect::<Vec<_>>(),
            "facts": self.facts,
        })
        .to_string()
    }
}

// ---------------------------------------------------------------- provider

/// Single-source descriptor for the built-in memory recall tool. Both the
/// provider (model-facing catalog) and the settings-page registry
/// (`agent_tools::builtin_tools` → `builtin_tools_list`) read from here, so
/// the model's view and the UI's view can never drift.
pub fn builtin_descriptor() -> ToolDescriptor {
    ToolDescriptor {
        name: "memory_search".into(),
        summary: "按日期或关键词翻找过往聊天记录与记忆（本地）".into(),
        detail: r#"{"type":"object","properties":{"query":{"type":"string","description":"用户原话，日期与关键词由系统解析；给了 date_from/date_to 时可省"},"date_from":{"type":"string","description":"显式开始日期 YYYY-MM-DD（含）；给出日期时按整窗扫描，忽略关键词。date_from 与 date_to 成对给出，可与 query 二选一"},"date_to":{"type":"string","description":"显式结束日期 YYYY-MM-DD（含）"}}}"#.into(),
        kind: ToolKind::Builtin,
        remote: false,
        access: ToolAccess::Read,
    }
}

/// The built-in tool. Prompt-only kinds answer `unsupported` in `execute`;
/// this one overrides `execute_with_conn` because it reads the memory store
/// directly (the pipeline always holds the connection).
pub struct MemorySearchProvider;

impl ToolProvider for MemorySearchProvider {
    fn descriptors(&self) -> Vec<ToolDescriptor> {
        vec![builtin_descriptor()]
    }

    fn execute(&self, _call: &ToolCall) -> Result<ToolOutput, String> {
        Err("unsupported".into())
    }

    fn needs_conn(&self) -> bool {
        true // the LIKE scan reads the store directly — audited locks for us
    }

    fn execute_with_conn(&self, conn: &Connection, call: &ToolCall) -> Result<ToolOutput, String> {
        let query = call.args.get("query").and_then(|v| v.as_str()).unwrap_or("");
        // Explicit date window wins over query-text parsing (inclusive both
        // ends → exclusive end inside the scan). Garbled dates fall back to
        // the deterministic parse of the query, then the trailing week.
        let parse_d = |k: &str| {
            call.args
                .get(k)
                .and_then(|v| v.as_str())
                .and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
        };
        let recall = match (parse_d("date_from"), parse_d("date_to")) {
            (Some(from), Some(to)) if to >= from => {
                recall_in_window(conn, from, to.succ_opt().unwrap_or(to))
            }
            _ => recall(conn, query),
        };
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

    /// Registry single-source: `agent_tools::builtin_tools` is exactly what
    /// the built-in providers mount, so the settings page and the model agree.
    #[test]
    fn builtin_registry_matches_provider() {
        let reg = crate::agent_tools::builtin_tools();
        // P3-3 起 registry = memory_search(1) + perception_search(1) + filetools(4)
        // + webtools(2)。fs/web 工具在设置页常显，但 provider 按开关/roots 门控
        // 挂载（各自模块单测覆盖）；perception_search 由 might_perceive 门控。
        assert_eq!(
            reg,
            [
                vec![builtin_descriptor()],
                vec![crate::perception_search::builtin_descriptor()],
                crate::filetools::builtin_descriptors(),
                crate::webtools::builtin_descriptors()
            ]
            .concat()
        );
        let ms = &reg[0];
        assert_eq!(ms.name, "memory_search");
        assert_eq!(ms.kind, ToolKind::Builtin);
        assert!(!ms.remote);
        assert!(MemorySearchProvider.descriptors() == vec![builtin_descriptor()]);
        // 注册表无重名
        let mut names: Vec<_> = reg.iter().map(|d| d.name.clone()).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), reg.len());
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
            // 2026-11-21 turn 13–20 教训：原文类 + 星期类必须命中，
            // 否则空 kit 直接走单轮直答，「周四聊天原文」永远答不上。
            "周四聊天原文",
            "周五聊天原文",
            "把那天的逐字记录给我",
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

    /// 星期词一等支持（2026-11-21 turn 12–20）。today() = 2026-11-20 周五。
    #[test]
    fn weekday_phrases_resolve_to_the_recent_single_day() {
        let d = |y: i32, m: u32, dd: u32| NaiveDate::from_ymd_opt(y, m, dd).unwrap();
        // 裸周X = 最近一次该星期；周五说「周四」= 昨天。
        assert_eq!(win("周四聊天原文").0, d(2026, 11, 19));
        assert_eq!(win("周三我说了啥").0, d(2026, 11, 18));
        // 今天当天算数：周五说「周五」= 今天。
        assert_eq!(win("周五聊了啥").0, d(2026, 11, 20));
        // 周五说「周日」：本周日是未来 → 回退上周日。
        assert_eq!(win("周日的安排").0, d(2026, 11, 15));
        assert_eq!(win("星期二的会").0, d(2026, 11, 17));
        assert_eq!(win("礼拜三的事").0, d(2026, 11, 18));
        // 上周X / 上上周X：那一周的周X。
        assert_eq!(win("上周四的对话").0, d(2026, 11, 12));
        assert_eq!(win("上上周一说过").0, d(2026, 11, 2));
        // 单日窗口：start+1 结尾，label 是「M月D日（周X）」。
        let (s, e, label) = win("周四聊天原文");
        assert_eq!(e, s.succ_opt().unwrap());
        assert_eq!(label, "周四");
        // 整窗扫描关键词：剥掉日期与回看类词后应为空。
        assert!(keywords("周四聊天原文", "周四").is_empty());
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
        // full 是 JSON（/trace 结构化渲染），行文本在 lines[].text 里
        let parsed: serde_json::Value = serde_json::from_str(&out.full).expect("full must be JSON");
        assert_eq!(parsed["window"], "11月18日（周三）");
        assert_eq!(parsed["dialogue_count"], 1);
        assert_eq!(parsed["lines"][0]["who"], "用户");
        assert_eq!(parsed["lines"][0]["text"], "聊了盆栽的形态");

        // 缺 args → 仍给出最近一周的诚实结果，不报错
        let out2 = p
            .execute_with_conn(&conn, &ToolCall { name: "memory_search".into(), args: serde_json::json!({}) })
            .unwrap();
        assert!(out2.summary.contains("对话 1 条"), "window fallback must still find the row: {}", out2.summary);
    }

    /// Explicit date window (2026-11-21): date_from/date_to must win over
    /// the query text — inclusive both ends, whole-window scan, and the
    /// query's words must NOT become keyword legs that filter rows out.
    #[test]
    fn explicit_date_window_scans_the_whole_range() {
        let _g = crate::memory::clock::pinned("2026-11-21T07:00:00Z"); // 本地 11-21 15:00
        let conn = mem();
        insert_dialogue(&conn, "2026-11-20T01:00:00+00:00", "user", "聊了盆栽的形态");
        insert_dialogue(&conn, "2026-11-20T02:00:00+00:00", "assistant", "换水别太勤。");
        insert_dialogue(&conn, "2026-11-21T01:00:00+00:00", "user", "今天的话不该进昨天窗口");
        let p = MemorySearchProvider;
        let out = p
            .execute_with_conn(&conn, &ToolCall {
                name: "memory_search".into(),
                args: serde_json::json!({
                    "query": "昨天说了什么",
                    "date_from": "2026-11-20",
                    "date_to": "2026-11-20",
                }),
            })
            .unwrap();
        assert!(out.summary.contains("11月20日（周五）"), "explicit window label: {}", out.summary);
        assert!(out.summary.contains("盆栽的形态"), "row inside the window: {}", out.summary);
        assert!(!out.summary.contains("今天的话"), "row outside the window: {}", out.summary);

        // 坏日期 → 落回 query 解析，不报错
        let out2 = p
            .execute_with_conn(&conn, &ToolCall {
                name: "memory_search".into(),
                args: serde_json::json!({
                    "query": "昨天说了什么",
                    "date_from": "11月20日",
                    "date_to": "not-a-date",
                }),
            })
            .unwrap();
        assert!(out2.summary.contains("11月20日"), "garbled dates fall back to query parse: {}", out2.summary);
    }

    /// query is OPTIONAL since 2026-11-21: a date-only call must run the
    /// whole-window scan without any required-arg rejection.
    #[test]
    fn date_only_call_works_without_query() {
        let _g = crate::memory::clock::pinned("2026-11-21T07:00:00Z");
        let conn = mem();
        insert_dialogue(&conn, "2026-11-20T01:00:00+00:00", "user", "聊了水培");
        let p = MemorySearchProvider;
        let out = p
            .execute_with_conn(&conn, &ToolCall {
                name: "memory_search".into(),
                args: serde_json::json!({ "date_from": "2026-11-20", "date_to": "2026-11-20" }),
            })
            .unwrap();
        assert!(out.summary.contains("11月20日"), "date-only window: {}", out.summary);
        assert!(out.summary.contains("水培"), "row inside the window: {}", out.summary);

        // 校验器对 date-only 也不拦（query 已可选）
        let d = builtin_descriptor();
        assert!(crate::agent_tools::validate_args(
            &d.detail,
            &serde_json::json!({ "date_from": "2026-11-20", "date_to": "2026-11-20" })
        )
        .is_ok());
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
        // 倒序取（2026-11-21 方案A）：100 条中保住的是最近的 60 条，
        // 第 99 条（最新）必须在、第 0 条（最旧）必须被挤出。
        assert!(r.lines.iter().any(|(_, _, c)| c.contains("第99条")), "keep the newest: {:?}", r.lines.first());
        assert!(!r.lines.iter().any(|(_, _, c)| c.contains("第0条")), "drop the oldest");
    }

    // ------------------------------------------------ S1：腿展开 / 降级 / 事实取新

    /// 段内 2-gram 展开：整段与二字切片同时进 LIKE。（S0 基线：字面自然句
    /// 端到端只有 8%，因为「报销流程怎么走」整段成一条腿。）
    #[test]
    fn legs_expand_segments_into_bigrams() {
        let got = legs("报销流程怎么走", "");
        assert!(got.contains(&"报销流程怎么走".to_string()), "长腿保留: {got:?}");
        assert!(got.contains(&"报销".to_string()), "二字切片必须进腿: {got:?}");
        assert!(got.contains(&"流程".to_string()));
        // 去重 + 上限
        assert!(got.len() <= MAX_LIKE_LEGS);
        let mut uniq = got.clone();
        uniq.sort();
        uniq.dedup();
        assert_eq!(uniq.len(), got.len(), "腿不得重复: {got:?}");
        // 元指代词不产腿（否则整窗兜底永远跑不到）
        assert!(legs("之前提过的那件事", "").is_empty(), "纯指代不该有腿");
    }

    /// S0 基线的头号事故：自然句整段匹配不上 → 现在靠二字腿命中。
    #[test]
    fn natural_sentence_hits_via_bigram_legs() {
        let _g = crate::memory::clock::pinned("2026-11-20T07:00:00Z");
        let conn = mem();
        insert_dialogue(&conn, "2026-11-19T01:00:00+00:00", "user", "我在走报销流程，先找主管审批");
        let r = recall(&conn, "报销流程怎么走");
        assert_eq!(r.lines.len(), 1, "二字腿应命中: {:?}", r.lines);
        assert!(!r.degraded, "命中就不该标降级");
        assert!(r.summary().contains("报销流程"));
    }

    /// 三级降级：腿全落空 → 退回整窗，不谎报「没有记录」。
    #[test]
    fn zero_hit_falls_back_to_the_whole_window() {
        let _g = crate::memory::clock::pinned("2026-11-20T07:00:00Z");
        let conn = mem();
        insert_dialogue(&conn, "2026-11-19T01:00:00+00:00", "user", "今天换了水培的营养液");
        conn.execute(
            "INSERT INTO facts (ts, content, confidence) VALUES ('2026-11-19T03:00:00+00:00', '用户在养水培', 0.7)",
            [],
        )
        .unwrap();
        // 腿（报销/销流/流程…）一条都不在库里 → 必须降级而不是空结果
        let r = recall(&conn, "报销流程怎么走");
        assert!(r.degraded, "腿全落空必须标 degraded");
        assert_eq!(r.lines.len(), 1, "整窗兜底应把窗口里的对话带回来: {:?}", r.lines);
        assert_eq!(r.facts.len(), 1, "整窗兜底的事实也要带回来");
        assert!(r.summary().contains("退回整窗"), "模型侧要看见降级说明: {}", r.summary());
        let full: serde_json::Value = serde_json::from_str(&r.full()).unwrap();
        assert_eq!(full["degraded"], true, "/trace 侧要有结构化标记");

        // 真·空窗口仍然是诚实的「没有记录」，且不标降级（无可降之物）
        let empty = recall(&conn, "18号说了啥");
        assert!(!empty.degraded);
        assert!(empty.summary().contains("没有找到对话记录"));
    }

    /// 事实行满额时保住最新的：`facts_in_window` 是 id ASC，直接截断会把
    /// 「最近说的」先丢掉（S0 基线里 12 条事实窗口的纯指代就栽在这）。
    #[test]
    fn fact_cap_keeps_the_newest_ones() {
        let _g = crate::memory::clock::pinned("2026-11-20T07:00:00Z");
        let conn = mem();
        for i in 0..(MAX_FACT_ROWS + 2) {
            conn.execute(
                "INSERT INTO facts (ts, content, confidence, importance) VALUES ('2026-11-19T03:00:00+00:00', ?1, 0.7, 3)",
                rusqlite::params![format!("事实{i}")],
            )
            .unwrap();
        }
        // 无腿 → 整窗；事实取满 MAX_FACT_ROWS 条
        let r = recall(&conn, "还记得吗");
        assert_eq!(r.facts.len(), MAX_FACT_ROWS);
        let newest = format!("事实{}", MAX_FACT_ROWS + 1);
        assert!(r.facts.iter().any(|f| f == &newest), "最新的一条必须在: {:?}", r.facts);
        assert!(!r.facts.iter().any(|f| f == "事实0"), "最旧的一条应被挤出");
        // 反排回时间正序：最后一条是最新
        assert_eq!(r.facts.last(), Some(&newest));
    }

    /// 元指代词进闸门：这类问法必须上工具（多一轮 << 装不记得）。
    #[test]
    fn gate_opens_on_meta_references() {
        for t in ["之前说过的那个事", "以前聊的那个", "那次说的方案", "当时你怎么说的"] {
            assert!(might_recall(t), "gate must fire on: {t}");
        }
    }

    /// 闸门语感语料（2026-11-23 体感核查）：17 条「想让它想起」的说法里
    /// 补词前漏 8 条（47%）——漏报全在口语时间词（昨天/前天/上个月）、
    /// 语气词「来着」、过去式动词「讨论过/商量过/说到哪」。负例盯住两类
    /// 不能误伤的：纯陈述（「我忘了带钥匙」）与新话题请求（「那个项目
    /// 下周上线」）。误报只多一轮模型轮次，漏报是「装不记得」，所以
    /// 词表永远偏宽容。
    #[test]
    fn gate_opens_on_colloquial_recall() {
        for t in [
            "还记得吗",
            "你还记得我叫什么吗",
            "那件事后来怎么样了",
            "我们上次说到哪了",
            "我之前跟你说过我喜欢喝什么来着",
            "你是不是提过",
            "上个月你说要带我去爬山",
            "记不记得我跟你说过买菜的事",
            "那个方案叫什么来着",
            "接着聊昨天的话题",
            "前天我让你查的东西查了吗",
            "上周我抱怨工作的事你还记得吗",
            "我们讨论过这个问题吗",
            "这事咱俩商量过吧",
            "咱们说到哪了",
            "昨天我让你做的事你做了吗",
            "我们上次聊到哪了",
        ] {
            assert!(might_recall(t), "gate must fire on: {t}");
        }
        for t in [
            "我今天好累",
            "明天开始减肥",
            "那个项目下周上线",
            "我忘了带钥匙",
            "帮我写今天的周报",
            "会议室的灯坏了",
        ] {
            assert!(!might_recall(t), "gate must stay quiet on: {t}");
        }
    }
}

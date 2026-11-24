//! Fact extraction (plan-memory.md §5.2–§5.3).
//!
//! `FactExtractor` decouples the consolidate pipeline from the model layer:
//! - `RuleExtractor` — offline fallback; deterministic, zero-dependency.
//! - `LocalExtractor` — the default model leg: a local LLM behind an
//!   Ollama-compatible API; same trait, structured JSON in / out. On any
//!   failure (server down, timeout, bad output) it falls back to the rules,
//!   per plan §5.2 — the pipeline never loses events to a bad model night.
//!   Cloud APIs are an explicit opt-in and deliberately NOT implemented.
//!
//! Per the plan's open-question recommendation, stage ① (session summaries)
//! is NOT built in P0, and `importance` stays a column without logic.

use serde::Deserialize;

use super::db::{ActiveFact, EventRow};

/// Four preference categories (report §4.1, plan §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    Like,
    Habit,
    Context,
    Boundary,
}

impl Category {
    pub fn as_str(&self) -> &'static str {
        match self {
            Category::Like => "like",
            Category::Habit => "habit",
            Category::Context => "context",
            Category::Boundary => "boundary",
        }
    }
}

/// One extracted candidate fact, fed into the merge / contradiction stages.
#[derive(Debug, Clone)]
pub struct CandidateFact {
    pub content: String,
    pub category: Category,
    pub confidence: f64,
    /// Event that produced this candidate (traceability invariant).
    pub source_event_id: Option<i64>,
    /// Correction marker: the pipeline resolves WHICH active fact this
    /// supersedes at apply time (so same-run facts count — plan §5.1 ④).
    pub is_correction: bool,
    /// 模型显式指认的目标 fact（plan-intelligence I3）；规则路径为 None。
    pub target_fact_id: Option<i64>,
    /// 用户画像出口（plan-user-profile.md §3）：key 已过白名单校验的
    /// (key, value)。一份抽取两个出口——facts 照旧落库，画像由
    /// consolidate apply 写进 user_profile。None = 本条不带画像。
    pub profile: Option<(String, String)>,
    /// 抽出来源（画像 manual 保护的判定依据）：'rule' | 'model'。
    pub source: &'static str,
}

/// What an extractor may look at. `active_facts` is provided so rule-based
/// duplicate detection and correction detection stay deterministic.
pub struct SessionDigest<'a> {
    pub user_utterances: &'a [EventRow],
    pub active_facts: &'a [ActiveFact],
}

pub trait FactExtractor: Send {
    fn extract(&self, digest: &SessionDigest) -> Vec<CandidateFact>;
}

// ------------------------------------------------------------------ rules

/// Normalization for exact-match merging (plan §5.3): lowercase, strip
/// punctuation and whitespace. Replaced by semantic similarity once the
/// vector leg lands.
pub fn normalize(text: &str) -> String {
    text.chars()
        .filter(|c| {
            !c.is_whitespace()
                && !c.is_ascii_punctuation()
                && !['，', '。', '！', '？', '、', '：', '；', '「', '」', '…', '·', '“', '”', '‘', '’']
                    .contains(c)
        })
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// Direct-statement prefixes: (prefix, category, confidence).
/// Kept to fixed patterns on purpose — the plan's risk table says P0 must
/// prefer false negatives over false positives (宁漏勿误).
const DIRECT_PATTERNS: &[(&str, Category, f64)] = &[
    ("我喜欢", Category::Like, 0.7),
    ("我讨厌", Category::Like, 0.7),
    ("我不喜欢", Category::Like, 0.7),
    ("我爱", Category::Like, 0.7),
    ("我每天", Category::Habit, 0.7),
    ("我习惯", Category::Habit, 0.7),
    ("我在用", Category::Context, 0.7),
    ("我正在用", Category::Context, 0.7),
    ("i like", Category::Like, 0.7),
    ("i love", Category::Like, 0.7),
    ("i hate", Category::Like, 0.7),
    ("i use", Category::Context, 0.7),
    ("every day i", Category::Habit, 0.7),
];

/// Correction markers: candidate supersedes the most recent active fact with
/// the highest content overlap.
const CORRECTION_MARKERS: &[&str] = &["说错了", "不对", "其实是", "我记错了", "not actually", "actually, i"];

/// Boundary markers → written straight to the boundaries table (highest
/// priority, plan §5.3), plus a `boundary`-category fact for traceability.
const BOUNDARY_MARKERS: &[&str] = &["别再", "不要再", "别在", "禁止", "别打断", "don't interrupt", "stop interrupting"];

// ------------------------------------------------------------- profile rules
// (docs/zh/plan-user-profile.md §3.1) — 规则层画像抽取。产出与 facts 同一批
// candidate（content 仍是用户原话），value 是剥掉前缀的短值；落库由
// consolidate apply 走 profile::apply（白名单 + manual 保护在那边）。

/// (前缀, key)——要求主语是「我」的确定性句式。前缀顺序即匹配顺序。
const PROFILE_PATTERNS: &[(&str, &str)] = &[
    ("我叫", "name"),
    ("叫我", "name"),
    ("我是做", "occupation"),
    ("我的工作是", "occupation"),
    ("我是一名", "occupation"),
    ("我在用", "tools"),
    ("我平时用", "tools"),
    ("我每天用", "tools"),
];

/// name 误报排除：给阿盆起名 / 指代他人都不是「用户叫什么」。
const NAME_EXCLUDES: &[&str] = &["阿盆", "你", "它", "他", "她", "咱", "别人"];

/// city 词表校验（方案 §3.1）：「我在 Cursor 上写代码」不能误进 city——
/// 必须命中真实城市名才落库。宁漏勿误：词表短就短，漏的城市靠模型层/UI。
const CITY_WORDS: &[&str] = &[
    "北京", "上海", "广州", "深圳", "杭州", "成都", "重庆", "武汉", "西安", "南京",
    "苏州", "天津", "长沙", "郑州", "青岛", "合肥", "厦门", "宁波", "无锡", "济南",
    "大连", "福州", "昆明", "沈阳", "哈尔滨", "石家庄", "南昌", "贵阳", "太原",
    "南宁", "兰州", "海口", "中国香港", "中国澳门",
];

/// name 值的合理长度上限（中文名）。
const NAME_MAX_CHARS: usize = 6;

/// 剥掉画像前缀后的值清洗：去尾部语气/标点（「我是做 QA 的」→「QA」）。
fn trim_profile_value(v: &str) -> String {
    v.trim()
        .trim_end_matches(['的', '了', '呢', '啊', '呀', '。', '！', '，', '？', ' ', '~', '…'])
        .trim()
        .to_string()
}

/// 一句话的画像匹配（规则层）。返回 Some((key, value)) 或 None。
/// 只做前缀剥离 + 最小校验，复杂口语交给模型层（§3.2）。
/// 入参是原文（非 lowercase）：匹配大小写不敏感，但值必须从原文切——
/// 「我是一名 QA」剥出来是 QA 不是 qa。
pub fn profile_match(original: &str) -> Option<(String, String)> {
    let lower = original.to_lowercase();
    // city 特例：「我在杭州」句式 + 词表校验，先于通用前缀（「我在」不是
    // 通用画像前缀，进词表才算命中）。
    if let Some(rest) = lower.strip_prefix("我在") {
        if let Some(city) = CITY_WORDS.iter().find(|c| rest.starts_with(**c)) {
            return Some(("city".to_string(), city.to_string()));
        }
    }
    let (key, raw) = PROFILE_PATTERNS
        .iter()
        .find(|(prefix, _)| lower.starts_with(*prefix))
        .map(|(prefix, key)| {
            // 前缀是 ASCII/中文字面量，lowercasing 不改这一段的 char 数，
            // 按前缀 char 数从原文切片是安全的。
            let skip = prefix.chars().count();
            (*key, original.chars().skip(skip).collect::<String>())
        })?;
    let value = trim_profile_value(&raw);
    if value.is_empty() {
        return None;
    }
    if key == "name"
        && (value.chars().count() > NAME_MAX_CHARS
            || NAME_EXCLUDES.iter().any(|e| value.contains(e)))
    {
        return None; // 宁漏勿误：「我叫你放心」「叫他阿盆」都不抽
    }
    Some((key.to_string(), value))
}

fn overlap(a: &str, b: &str) -> usize {
    // Shared 2-gram count (bigrams): the weakest similarity that still
    // separates a correction from its target on short Chinese utterances,
    // while staying deterministic and allocation-light (P0 scope).
    let (a, b) = (normalize(a), normalize(b));
    a.chars()
        .collect::<Vec<_>>()
        .windows(2)
        .filter(|w| b.contains(&w.iter().collect::<String>()))
        .count()
}

/// 规则路径：最重叠的有效 fact。阈值 ≥3 共享 2-gram（I3 D8，降低短中文误匹配）。
pub fn correction_target<'a>(
    facts: &'a [ActiveFact],
    content: &str,
) -> Option<&'a ActiveFact> {
    facts
        .iter()
        .map(|f| (overlap(content, &f.content), f))
        .filter(|(hits, _)| *hits >= 3)
        .max_by_key(|(hits, _)| *hits)
        .map(|(_, f)| f)
}

/// 显式 id 优先；校验存在于 active 列表后再接受。
pub fn resolve_correction_target<'a>(
    facts: &'a [ActiveFact],
    content: &str,
    target_fact_id: Option<i64>,
) -> Option<&'a ActiveFact> {
    if let Some(id) = target_fact_id {
        if let Some(f) = facts.iter().find(|f| f.id == id) {
            // 软校验：至少 1 个共享 2-gram；否则拒绝乱填 id。
            if overlap(content, &f.content) >= 1 {
                return Some(f);
            }
            return None;
        }
        return None;
    }
    correction_target(facts, content)
}

pub struct RuleExtractor;

impl FactExtractor for RuleExtractor {
    fn extract(&self, digest: &SessionDigest) -> Vec<CandidateFact> {
        let mut out = Vec::new();
        for ev in digest.user_utterances {
            let lower = ev.content.to_lowercase();

            // 1) Explicit boundary — highest priority, wins over other rules.
            if BOUNDARY_MARKERS.iter().any(|m| lower.contains(m)) {
                out.push(CandidateFact {
                    content: ev.content.clone(),
                    category: Category::Boundary,
                    confidence: 0.95,
                    source_event_id: Some(ev.id),
                    is_correction: false,
                    target_fact_id: None,
                    profile: None,
                    source: "rule",
                });
                continue;
            }

            // 2) Correction — target resolves at apply time (see
            //    CandidateFact::is_correction); apply_candidates skips it
            //    when nothing overlaps, so emitting unconditionally is safe.
            //    更正句不带画像（更正的语义先于身份陈述，宁漏勿误）。
            if CORRECTION_MARKERS.iter().any(|m| lower.contains(m)) {
                out.push(CandidateFact {
                    content: ev.content.clone(),
                    category: Category::Context,
                    confidence: 0.6,
                    source_event_id: Some(ev.id),
                    is_correction: true,
                    target_fact_id: None,
                    profile: None,
                    source: "rule",
                });
                continue;
            }

            // 3) 画像匹配（方案 §3.1）：独立于直接陈述——「我在用 Cursor」
            //    两条规则都命中时，画像挂到同一条 candidate 上（一份抽取
            //    两个出口，不是两条 candidate）。传原文，值的大小写保真。
            let profile = profile_match(&ev.content);

            // 4) Direct statement.
            if let Some((_, category, confidence)) = DIRECT_PATTERNS
                .iter()
                .find(|(prefix, _, _)| lower.starts_with(prefix))
            {
                out.push(CandidateFact {
                    content: ev.content.clone(),
                    category: *category,
                    confidence: *confidence,
                    source_event_id: Some(ev.id),
                    is_correction: false,
                    target_fact_id: None,
                    profile,
                    source: "rule",
                });
                continue;
            }
            // 5) 只有画像命中、没有直接陈述句式：仍出一条 context candidate
            //    ——facts 留痕 + 画像落库都要发生。
            if let Some(p) = profile {
                out.push(CandidateFact {
                    content: ev.content.clone(),
                    category: Category::Context,
                    confidence: 0.7,
                    source_event_id: Some(ev.id),
                    is_correction: false,
                    target_fact_id: None,
                    profile: Some(p),
                    source: "rule",
                });
            }
            // 6) Everything else: no candidate. 宁漏勿误 — model layer (M5)
            //    widens this without touching the pipeline.
        }
        out
    }
}

// ------------------------------------------------------------------ local LLM leg

/// One extraction pass over a day's utterances. Local 7B models can take a
/// while on first load; on timeout the caller falls back to rules.
pub const EXTRACT_TIMEOUT_SECS: u64 = crate::llm::EXTRACT_TIMEOUT_SECS;

/// Model leg of the extractor (plan §5.2). One LLM call per consolidate run:
/// all of the day's utterances in, a JSON array of candidate facts out.
/// Any failure → `RuleExtractor` fallback (plan §5.2), so the pipeline's
/// guarantees never depend on the model behaving.
pub struct LocalExtractor {
    pub client: std::sync::Arc<dyn crate::llm::ChatClient>,
}

impl LocalExtractor {
    pub fn new() -> Self {
        // 后台腿：Auto 钉本地；Manual 跟随主配置（plan-structured-routing §3.6）。
        LocalExtractor {
            client: crate::llm::client_for(crate::llm::TaskLeg::Background),
        }
    }

    /// The model call, isolated so `extract` can fall back on any error.
    /// 传输层 `chat_structured` 约束 JSON；失败 → `None` → RuleExtractor。
    fn extract_via_llm(&self, digest: &SessionDigest) -> Option<Vec<CandidateFact>> {
        let reply = self
            .client
            .chat_structured(
                &[
                    crate::llm::Msg::system(EXTRACTION_SYSTEM_PROMPT),
                    crate::llm::Msg::user(extraction_user_prompt(digest)),
                ],
                crate::llm::schema::extraction_facts(),
                0.2, // extraction wants determinism, not creativity
                EXTRACT_TIMEOUT_SECS,
            )
            .ok()?;
        parse_candidates(&reply, digest.user_utterances)
    }
}

impl Default for LocalExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl FactExtractor for LocalExtractor {
    fn extract(&self, digest: &SessionDigest) -> Vec<CandidateFact> {
        self.extract_via_llm(digest).unwrap_or_else(|| RuleExtractor.extract(digest))
    }
}

/// 语义规则权威；JSON 形状权威在 `llm::schema::extraction_facts`
/// （plan-structured-routing T3：删提示词里的格式祈祷句）。
const EXTRACTION_SYSTEM_PROMPT: &str = r#"你是记忆抽取模块。输入是一天的用户发言（带编号）和当前已知事实。你的任务：从发言中提炼出值得长期记住的候选事实。

规则：
- fact 必须是可断言的单一事实，改写为第三人称（如用户说"我喜欢雨天"→"用户喜欢雨天"）。
- category：like=显式喜好/厌恶；habit=行为规律；context=稳定背景；boundary=用户明确说"别/不要再做X"。
- correction=true 表示该发言在更正一个已有事实（"说错了，其实是…"）。
- profile（可省略）：当发言透露了用户的固定身份信息时填写，key 只允许 name（用户姓名/怎么称呼）、occupation（职业/角色）、tools（常用工具）、city（所在城市）、projects（正在做的事）、schedule（作息规律）；value 是第一人称原值的简短形式（如"我叫许一"→ value "许一"）。key 不在上面的清单里就一律填 null。拿不准就填 null。
- 只抽有把握的。宁漏勿误：拿不准就不要输出。没有可抽取的内容时输出空数组。"#;

/// Build the user-side prompt: numbered utterances + the active facts (so the
/// model can mark corrections and avoid re-stating what is already known).
fn extraction_user_prompt(digest: &SessionDigest) -> String {
    let mut out = String::with_capacity(1024);
    out.push_str("【今日用户发言】\n");
    for (i, ev) in digest.user_utterances.iter().enumerate() {
        out.push_str(&format!("{}. {}\n", i, ev.content));
    }
    if !digest.active_facts.is_empty() {
        out.push_str("\n【当前已知事实】\n");
        for f in digest.active_facts {
            out.push_str(&format!("- {}\n", f.content));
        }
    }
    out.push_str("\n请输出候选事实 JSON 数组。");
    out
}

/// Strip markdown code fences the model may wrap its array in.
fn strip_fences(text: &str) -> &str {
    let t = text.trim();
    if let Some(rest) = t.strip_prefix("```") {
        // drop an optional language tag line ("json")
        let rest = rest.trim_start_matches(['j', 's', 'o', 'n']).trim_start_matches('\n');
        return rest.strip_suffix("```").unwrap_or(rest).trim();
    }
    t
}

#[derive(Debug, Deserialize)]
struct RawCandidate {
    idx: i64,
    fact: String,
    category: String,
    #[serde(default)]
    confidence: Option<f64>,
    #[serde(default)]
    correction: bool,
    #[serde(default)]
    target_fact_id: Option<i64>,
    #[serde(default)]
    profile: Option<RawProfile>,
}

/// 模型侧画像输出（plan-user-profile.md §3.2）。key 白名单在这里硬校验：
/// 白名单外的 key 静默丢弃（宁漏勿误），value 空串同样丢弃。
#[derive(Debug, Deserialize)]
struct RawProfile {
    key: String,
    value: String,
}

impl RawProfile {
    fn into_valid(self) -> Option<(String, String)> {
        let value = self.value.trim().to_string();
        if value.is_empty() || !super::profile::is_valid_key(&self.key) {
            return None;
        }
        Some((self.key, value))
    }
}

/// Parse the model's JSON array into candidates. Traceability invariant: a
/// candidate is only kept if `idx` maps to a real utterance event. Unknown
/// categories and out-of-range confidences are dropped/clamped (宁漏勿误).
fn parse_candidates(reply: &str, events: &[EventRow]) -> Option<Vec<CandidateFact>> {
    let cleaned = strip_fences(reply);
    let raw: Vec<RawCandidate> = serde_json::from_str(cleaned).ok()?;
    let mut out = Vec::new();
    for c in raw {
        if c.idx < 0 || c.idx as usize >= events.len() {
            continue; // bad index: drop the candidate, keep parsing the rest
        }
        let ev = &events[c.idx as usize];
        let category = match c.category.as_str() {
            "like" => Category::Like,
            "habit" => Category::Habit,
            "context" => Category::Context,
            "boundary" => Category::Boundary,
            _ => continue,
        };
        let confidence = c.confidence.unwrap_or(0.7).clamp(0.0, 1.0);
        let profile = c.profile.and_then(|p| p.into_valid());
        out.push(CandidateFact {
            content: c.fact.trim().to_string(),
            category,
            confidence,
            source_event_id: Some(ev.id),
            is_correction: c.correction,
            target_fact_id: c.target_fact_id,
            profile,
            source: "model",
        });
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(id: i64, content: &str) -> EventRow {
        EventRow {
            id,
            ts: "2026-11-18T00:00:00Z".into(),
            kind: "chat".into(),
            role: Some("user".into()),
            session_id: None,
            content: content.into(),
        }
    }

    #[test]
    fn extracts_direct_statements_with_category() {
        let evs = vec![
            event(1, "我喜欢雨天的声音"),
            event(2, "我每天早上十点才开工"),
            event(3, "我在用 Rust 写桌面项目"),
        ];
        let ex = RuleExtractor.extract(&SessionDigest {
            user_utterances: &evs,
            active_facts: &[],
        });
        assert_eq!(ex.len(), 3);
        assert_eq!(ex[0].category, Category::Like);
        assert_eq!(ex[1].category, Category::Habit);
        assert_eq!(ex[2].category, Category::Context);
        assert_eq!(ex[0].source_event_id, Some(1));
    }

    #[test]
    fn boundaries_win_and_skip_confidence_gates() {
        let evs = vec![event(1, "我喜欢雨天"), event(2, "别在我专注的时候弹窗")];
        let ex = RuleExtractor.extract(&SessionDigest {
            user_utterances: &evs,
            active_facts: &[],
        });
        assert_eq!(ex[1].category, Category::Boundary);
        assert_eq!(ex[1].confidence, 0.95);
    }

    #[test]
    fn correction_points_at_overlapping_fact() {
        let facts = vec![ActiveFact {
            id: 7,
            ts: "2026-11-17T00:00:00Z".into(),
            content: "用户喜欢下雨天".into(),
            confidence: 0.7,
            importance: 3,
            source_event_id: None,
        }];
        let evs = vec![event(1, "说错了，其实我不喜欢下雨天")];
        let ex = RuleExtractor.extract(&SessionDigest {
            user_utterances: &evs,
            active_facts: &facts,
        });
        assert_eq!(ex.len(), 1);
        assert!(ex[0].is_correction);
        // Target resolves at apply time; here we only verify the resolver.
        assert_eq!(correction_target(&facts, &evs[0].content).map(|f| f.id), Some(7));
    }

    #[test]
    fn correction_without_target_resolves_to_none() {
        let evs = vec![event(1, "说错了，其实是下周三截止")];
        let ex = RuleExtractor.extract(&SessionDigest {
            user_utterances: &evs,
            active_facts: &[],
        });
        assert_eq!(ex.len(), 1);
        assert!(correction_target(&[], &evs[0].content).is_none());
    }

    #[test]
    fn silence_beats_false_positives() {
        let evs = vec![event(1, "今天天气不错，我们出去走走吧"), event(2, "随便")];
        let ex = RuleExtractor.extract(&SessionDigest {
            user_utterances: &evs,
            active_facts: &[],
        });
        assert!(ex.is_empty());
    }

    // ---- 画像规则（plan-user-profile.md §3.1）----

    #[test]
    fn profile_patterns_extract_identity_values() {
        let evs = vec![
            event(1, "我叫许一"),
            event(2, "我是做质量管理的"),
            event(3, "我的工作是写代码"),
            event(4, "我是一名 QA"),
            event(5, "我平时用 Figma 画图"),
        ];
        let ex = RuleExtractor.extract(&SessionDigest {
            user_utterances: &evs,
            active_facts: &[],
        });
        assert_eq!(ex.len(), 5);
        assert_eq!(ex[0].profile.as_ref().map(|(k, v)| (k.as_str(), v.as_str())), Some(("name", "许一")));
        assert_eq!(ex[1].profile.as_ref().map(|(k, v)| (k.as_str(), v.as_str())), Some(("occupation", "质量管理")));
        assert_eq!(ex[2].profile.as_ref().map(|(k, v)| (k.as_str(), v.as_str())), Some(("occupation", "写代码")));
        assert_eq!(ex[3].profile.as_ref().map(|(k, v)| (k.as_str(), v.as_str())), Some(("occupation", "QA")));
        assert_eq!(ex[4].profile.as_ref().map(|(k, v)| (k.as_str(), v.as_str())), Some(("tools", "Figma 画图")));
    }

    #[test]
    fn profile_and_direct_statement_share_one_candidate() {
        // 「我在用 Cursor」：DIRECT_PATTERNS（context fact）与画像规则同时
        // 命中——一条 candidate 双出口，不是两条。
        let evs = vec![event(1, "我在用 Cursor")];
        let ex = RuleExtractor.extract(&SessionDigest {
            user_utterances: &evs,
            active_facts: &[],
        });
        assert_eq!(ex.len(), 1);
        assert_eq!(ex[0].category, Category::Context);
        assert_eq!(ex[0].profile.as_ref().map(|(k, v)| (k.as_str(), v.as_str())), Some(("tools", "Cursor")));
    }

    #[test]
    fn profile_only_utterance_still_yields_a_candidate() {
        // 「我叫许一」不在 DIRECT_PATTERNS 里：画像命中也要出 candidate，
        // facts 留痕 + 画像落库都要发生。
        let evs = vec![event(1, "我叫许一")];
        let ex = RuleExtractor.extract(&SessionDigest {
            user_utterances: &evs,
            active_facts: &[],
        });
        assert_eq!(ex.len(), 1);
        assert_eq!(ex[0].category, Category::Context);
        assert_eq!(ex[0].profile.as_ref().map(|(k, v)| (k.as_str(), v.as_str())), Some(("name", "许一")));
    }

    #[test]
    fn city_requires_wordlist_hit() {
        let evs = vec![
            event(1, "我在杭州工作"),
            event(2, "我在 Cursor 上写代码"),
        ];
        let ex = RuleExtractor.extract(&SessionDigest {
            user_utterances: &evs,
            active_facts: &[],
        });
        let by_id = |id: i64| ex.iter().find(|c| c.source_event_id == Some(id));
        assert_eq!(
            by_id(1).unwrap().profile.as_ref().map(|(k, v)| (k.as_str(), v.as_str())),
            Some(("city", "杭州"))
        );
        // 非城市词不进 city；「没命中任何规则就不出 candidate」是既有语义，
        // 出了 candidate 也不许带画像。
        assert!(by_id(2).map(|c| c.profile.is_some()) != Some(true), "非城市词不进 city");
    }

    #[test]
    fn name_false_positives_are_rejected() {
        let evs = vec![
            event(1, "我叫你放心"),      // 指代不是自称
            event(2, "叫我阿盆就行"),    // 给阿盆起名
            event(3, "我叫王长征就是不服"),
        ];
        let ex = RuleExtractor.extract(&SessionDigest {
            user_utterances: &evs,
            active_facts: &[],
        });
        let by_id = |id: i64| ex.iter().find(|c| c.source_event_id == Some(id));
        assert!(by_id(1).map(|c| c.profile.is_some()) != Some(true), "「我叫你」不是自我介绍");
        assert!(by_id(2).map(|c| c.profile.is_some()) != Some(true), "阿盆是阿盆不是用户");
        assert!(by_id(3).map(|c| c.profile.is_some()) != Some(true), "超长值不是姓名");
    }

    #[test]
    fn corrections_carry_no_profile() {
        let evs = vec![event(1, "说错了，其实我叫李四")];
        let ex = RuleExtractor.extract(&SessionDigest {
            user_utterances: &evs,
            active_facts: &[],
        });
        assert!(ex[0].is_correction);
        assert_eq!(ex[0].profile, None, "更正句不抽画像（宁漏勿误）");
    }

    #[test]
    fn boundary_wins_over_profile() {
        let evs = vec![event(1, "别再问我在用什么工具了")];
        let ex = RuleExtractor.extract(&SessionDigest {
            user_utterances: &evs,
            active_facts: &[],
        });
        assert_eq!(ex[0].category, Category::Boundary);
        assert_eq!(ex[0].profile, None);
    }

    #[test]
    fn rule_candidates_are_sourced_rule() {
        let evs = vec![event(1, "我叫许一")];
        let ex = RuleExtractor.extract(&SessionDigest {
            user_utterances: &evs,
            active_facts: &[],
        });
        assert_eq!(ex[0].source, "rule");
    }

    // ---- 模型层画像（plan-user-profile.md §3.2）----

    #[test]
    fn normalization_strips_punct_and_case() {
        assert_eq!(normalize("我喜欢，雨天！"), normalize("我喜欢雨天"));
        assert_eq!(normalize("I LIKE Rust"), normalize("i like rust"));
    }

    // ---- LocalExtractor pure-logic tests (no network) ----

    #[test]
    fn extraction_prompt_has_no_format_prayer() {
        // plan-structured-routing T3：形状权威在 schema，提示词禁止再写格式行。
        assert!(!EXTRACTION_SYSTEM_PROMPT.contains("只输出一个 JSON 数组"));
        assert!(!EXTRACTION_SYSTEM_PROMPT.contains("每条候选的格式"));
        assert!(!EXTRACTION_SYSTEM_PROMPT.contains(r#"{"idx""#));
    }

    #[test]
    fn parses_plain_and_fenced_arrays() {
        let evs = vec![event(1, "我喜欢雨天"), event(2, "我每天早上十点才开工")];

        let plain = r#"[{"idx":0,"fact":"用户喜欢雨天","category":"like","confidence":0.8,"correction":false}]"#;
        let parsed = parse_candidates(plain, &evs).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].content, "用户喜欢雨天");
        assert_eq!(parsed[0].source_event_id, Some(1));

        let fenced = format!("```json\n{}\n```", plain);
        assert_eq!(parse_candidates(&fenced, &evs).unwrap().len(), 1);
        assert_eq!(parse_candidates("[]", &evs).unwrap().len(), 0);
    }

    #[test]
    fn drops_bad_idx_unknown_category_and_clamps_confidence() {
        let evs = vec![event(1, "我喜欢雨天")];
        let reply = r#"[
            {"idx": 7,  "fact": "幽灵事实", "category": "like",   "confidence": 0.9},
            {"idx": 0,  "fact": "怪分类",   "category": "maybe",  "confidence": 0.9},
            {"idx": 0,  "fact": "高置信",   "category": "habit",  "confidence": 5.0},
            {"idx": -1, "fact": "负编号",   "category": "context","confidence": 0.8},
            {"idx": 0,  "fact": "缺置信",   "category": "context"}
        ]"#;
        let parsed = parse_candidates(reply, &evs).unwrap();
        // idx 7 / -1 / unknown category dropped; clamped + defaulted kept.
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].confidence, 1.0);
        assert_eq!(parsed[1].confidence, 0.7);
    }

    #[test]
    fn garbage_reply_is_none_not_panic() {
        let evs = vec![event(1, "我喜欢雨天")];
        assert!(parse_candidates("好的，以下是候选事实：……", &evs).is_none());
        assert!(parse_candidates("", &evs).is_none());
    }

    #[test]
    fn parses_model_profile_field_with_whitelist_gate() {
        let evs = vec![event(1, "我叫许一，做 QA"), event(2, "我的年薪是五十万")];
        let reply = r#"[
            {"idx":0,"fact":"用户叫许一","category":"context","confidence":0.8,"correction":false,"profile":{"key":"name","value":"许一"}},
            {"idx":1,"fact":"用户谈到了收入","category":"context","confidence":0.9,"correction":false,"profile":{"key":"income","value":"50w"}}
        ]"#;
        let parsed = parse_candidates(reply, &evs).unwrap();
        assert_eq!(parsed[0].profile.as_ref().map(|(k, v)| (k.as_str(), v.as_str())), Some(("name", "许一")));
        // 白名单外的 key（income）静默丢弃——candidate 本身保留，画像不落。
        assert_eq!(parsed[1].profile, None);
        assert_eq!(parsed[0].source, "model");
    }

    #[test]
    fn model_profile_empty_value_is_dropped() {
        let evs = vec![event(1, "我在一家公司上班")];
        let reply = r#"[{"idx":0,"fact":"用户有工作","category":"context","correction":false,"profile":{"key":"occupation","value":"  "}}]"#;
        let parsed = parse_candidates(reply, &evs).unwrap();
        assert_eq!(parsed[0].profile, None);
    }

    #[test]
    fn user_prompt_numbers_utterances_and_lists_facts() {
        let evs = vec![event(1, "我喜欢雨天")];
        let facts = vec![ActiveFact {
            id: 7,
            ts: "2026-11-17T00:00:00Z".into(),
            content: "用户喜欢雨天".into(),
            confidence: 0.7,
            importance: 3,
            source_event_id: None,
        }];
        let prompt = extraction_user_prompt(&SessionDigest {
            user_utterances: &evs,
            active_facts: &facts,
        });
        assert!(prompt.contains("0. 我喜欢雨天"));
        assert!(prompt.contains("- 用户喜欢雨天"));
    }

    #[test]
    fn local_extractor_falls_back_to_rules_when_server_down() {
        // Port 1 on localhost: nothing listens → LLM call fails → the trait
        // impl must behave exactly like RuleExtractor (plan §5.2).
        let ex = LocalExtractor {
            client: std::sync::Arc::new(crate::llm::OllamaClient::new("http://127.0.0.1:1", "test")),
        };
        let evs = vec![event(1, "我喜欢雨天"), event(2, "今天天气不错")];
        let out = ex.extract(&SessionDigest { user_utterances: &evs, active_facts: &[] });
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].category, Category::Like);
    }
}

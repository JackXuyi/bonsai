//! S0/S1 检索基线（2026-11-23）：向量检索决策的量化前置。
//!
//! 决策背景（对 C3「记忆与个性化」的核查）：`memory_search` 现在只有
//! 「日期窗口 + 关键词子串」一条腿，`db.rs:358` 的向量 TODO 与 roadmap
//! G-13 / P5-1 都写着「等触发条件」。要判断 embedding 值不值，先得把
//! 「现在到底丢在哪一步」量成数字——这份基线就是这个数字。
//!
//! 五类问法，三个指标：
//!
//! | 类别 | 问题形态 | 用来隔离哪一段 |
//! | --- | --- | --- |
//! | 精确片段 | 问题本身就是存过的原文片段 | 链路本身能不能跑通（能力上限） |
//! | 字面自然句 | 含原文词，但裹在自然句里 | 切词（`keywords_with`）损失 |
//! | 口语指代句 | 「上次说的X…」 | 切词 + 脚手架词残留 |
//! | 近义改写 | 与原文无公共二字切片 | 纯语义缺口（只有 embedding 能救的量） |
//! | 纯指代 | 「还记得吗」 | 无信号时的兜底上限 |
//!
//! 指标 A「腿纯净率」= 抽出的腿里至少有一条是**目标话题原文**子串的比例。
//! 指标 B「命中率」= 模型在召回结果里看到该话题证据的比例（对话行或事实行）。
//! 指标 C「走兜底率」= 这次是不是走了「腿全落空 → 整窗兜底」——B 里有多少
//! 是碰巧撞进窗口，看这一列。
//!
//! ## 基线（固定时钟 2026-12-01）
//!
//! S0（2026-11-23，小语料：12 对话 / 12 事实）：
//!
//! ```text
//! 问法          用例  腿纯净  命中
//! 精确片段      12    92%     100%
//! 字面自然句    12    8%      8%
//! 口语指代句    12    33%     33%
//! 近义改写      12    0%      0%
//! 纯指代        4     0%      50%
//! ```
//!
//! S1（同日，语料提到真实密度：+60 条噪声对话 / +40 条噪声事实）：
//!
//! ```text
//! 问法          用例  腿纯净  命中  走兜底
//! 精确片段      12    100%    100%  0%
//! 字面自然句    12    100%    100%  0%
//! 口语指代句    12    100%    100%  0%
//! 近义改写      12    0%      0%    83%
//! 纯指代        4     0%      0%    0%
//! ```
//!
//! **语料为什么变了**：S1 的整窗兜底一上线，12 条对话的小语料会把整个窗口
//! 倒回来，近义改写凭空「92% 命中」——那是兜底把答案免费送上门，不是召回
//! 能力。真实一周是几百条对话、几十条事实，所以 S0 的「精确片段 100% /
//! 纯指代 50%」也有同样的虚高成分。两张表不可混比，只作演进记录。
//!
//! S2（2026-11-23，FTS5 索引 + `beyond` 跨时间腿，语料同上 + 4 条 90 天前
//! 的事实）：
//!
//! ```text
//! 问法          用例  腿纯净  命中  走兜底
//! 精确片段      12    100%    100%  0%
//! 字面自然句    12    100%    100%  0%
//! 口语指代句    12    100%    100%  0%
//! 近义改写      12    0%      8%    83%
//! 纯指代        4     0%      0%    0%
//! 久远事实      4     100%    100%  50%
//! ```
//!
//! S2 换来的唯一大项是**久远事实 0% → 100%**：默认窗口只有最近 7 天，
//! 「三个月前说过我对花生过敏」原先压根搜不到（不是排序问题，是没有通路）。
//! 近义改写从 0 变成 1/12 是索引的 1 字 token 蹭到的，语义缺口当时仍在。
//!
//! S3（2026-11-24，查询侧同义簇展开，语料同上）：
//!
//! ```text
//! 问法          用例  腿纯净  命中  走兜底
//! 精确片段      12    100%    100%  0%
//! 字面自然句    12    100%    100%  0%
//! 口语指代句    12    100%    100%  0%
//! 近义改写      12    83%     100%  8%
//! 纯指代        4     0%      0%    0%
//! 久远事实      4     100%    100%  50%
//! ```
//!
//! S3 把近义命中 1/12 → 12/12：不靠 embedding，靠 `memory/synonym.rs` 在
//! 查询侧把「退钱/慢跑/坚果…」展开成库里的「报销/跑步/花生…」。OR 进腿，
//! 索引正文不动。embedding（P5-1）触发条件「近义 <3/12」已不成立；纯指代
//! 仍是产品取舍，不靠检索腿。
//!
//! ## 归因（S3 后）
//! 1. **近义改写**：通用同义簇覆盖生活记忆常用桥；簇外冷门改写仍可能漏，
//!    那才是将来 embedding 的正当靶子。
//! 2. **纯指代 0%**：无内容词，整窗/突显事实都不够——facts 常驻注入或靠
//!    上下文，不靠检索腿。
//! 3. **已修复**：字面/口语/精确（S1）、久远事实（S2）、近义常用桥（S3）。
//!
//! 只在测试构建里编译：这是量尺，不是产品功能。跑法：
//! `cargo test --lib recall_baseline -- --nocapture`

use rusqlite::Connection;

use super::db;
use crate::memory_search;

/// 一个话题：对话原文 + 被 consolidate 抽出的事实 + 断言的证据词 +
/// 三类问法。`expect` 必须同时是 `utterance` 与 `fact` 的子串，否则
/// 「命中」判定本身就不可信。
struct Topic {
    utterance: &'static str,
    fact: &'static str,
    expect: &'static str,
    literal: &'static str,
    colloquial: &'static str,
    synonym: &'static str,
}

/// 12 个话题（覆盖用户真实生活面：工作流程、家里装修、技术学习、生活习惯）。
///
/// 语料约束（两条都是量尺可信的前提）：
/// 1. `synonym` 必须与 `utterance`/`fact` **没有任何公共二字切片**——否则量到的
///    仍是字面命中，语义缺口会被自己编的题填平。
/// 2. 12 条事实 + 60 条噪声对话 + 40 条噪声事实：真实一周是几百条对话、几十条
///    事实，语料量不够时「整窗兜底」会把全部内容倒回来，每个类都虚高。
const TOPICS: [Topic; 12] = [
    Topic {
        utterance: "我在走报销流程，先找直属主管审批再去财务系统",
        fact: "用户在走报销流程，先找主管审批再进财务系统",
        expect: "报销流程",
        literal: "报销流程怎么走",
        colloquial: "上次说的报销流程到哪一步了",
        synonym: "退钱的手续卡在谁那儿了",
    },
    Topic {
        utterance: "我家客厅阳台想封窗，尺寸已经量过了",
        fact: "用户家客厅阳台准备封窗，尺寸已量",
        expect: "封窗",
        literal: "封窗尺寸量了吗",
        colloquial: "之前那个封窗的事有进展吗",
        synonym: "外推玻璃安装要多少钱",
    },
    Topic {
        utterance: "我喜欢雨天跑步，一般晚上八点出门",
        fact: "用户喜欢雨天跑步，晚上八点出门",
        expect: "雨天跑步",
        literal: "雨天跑步几点出门",
        colloquial: "上次聊的那个雨天跑步几点来着",
        synonym: "下着雨出去慢跑的习惯",
    },
    Topic {
        utterance: "我在杭州余杭看二手房，预算三百万左右",
        fact: "用户在杭州余杭看二手房，预算三百万",
        expect: "二手房",
        literal: "二手房预算多少",
        colloquial: "之前那个二手房的预算还记得吗",
        // R3：房子/二手房 为上下位，不再扩腿；改走「看房/预算」真近义桥。
        synonym: "看房预算上限卡在哪儿",
    },
    Topic {
        utterance: "周末想和女朋友自驾去舟山",
        fact: "用户周末想和女朋友自驾去舟山",
        expect: "自驾去舟山",
        literal: "自驾去舟山要多久",
        colloquial: "上次说的自驾去舟山还去吗",
        // R3：舟山/海边 不再扩；改走 开车/对象 真近义。
        synonym: "开车带对象出游那事怎样了",
    },
    Topic {
        utterance: "我在做一个复盘小程序，用毛泽东方法论",
        fact: "用户在做一个复盘小程序，用毛泽东方法论",
        expect: "复盘小程序",
        literal: "复盘小程序用什么方法论",
        colloquial: "那个复盘小程序做到哪了",
        // R3：小程序/应用 不再扩；毛选+反思仍为 EntityAlias/Synonym。
        synonym: "毛选式反思做到哪一步了",
    },
    Topic {
        utterance: "我对花生过敏，点菜要避开",
        fact: "用户对花生过敏，点菜要避开",
        expect: "花生过敏",
        literal: "花生过敏点菜注意什么",
        colloquial: "上次说的花生过敏还记得吧",
        // R3：花生/坚果、过敏/不能吃 禁止当同义扩；改写为仍无公共二字
        // 切片、但靠其它真近义无法桥接的问法——本条近义命中预期下降，
        // 见 baseline_numbers 棘轮注释。
        synonym: "坚果敏感受不了外出点餐怎么办",
    },
    Topic {
        utterance: "我每天早上喝一杯黑咖啡，不加糖",
        fact: "用户每天早上喝一杯黑咖啡，不加糖",
        expect: "黑咖啡",
        literal: "黑咖啡加不加糖",
        colloquial: "之前说的黑咖啡怎么喝来着",
        // R3：蔗糖不得与不加糖共簇；深色饮品→黑咖啡仍为 Synonym。
        synonym: "起床后那杯深色饮品还在喝吗",
    },
    Topic {
        utterance: "我家猫叫团子，今年三岁",
        fact: "用户的猫叫团子，今年三岁",
        expect: "团子",
        literal: "猫叫团子几岁",
        colloquial: "上次说的猫叫团子多大了",
        synonym: "家里养的小动物现在多大了",
    },
    Topic {
        utterance: "我在学 Rust，卡在所有权和借用",
        fact: "用户在学 Rust，卡在所有权和借用",
        expect: "所有权",
        literal: "Rust 卡在所有权",
        colloquial: "之前说的 Rust 学到哪了",
        synonym: "系统编程语言那门课遇到的难点",
    },
    Topic {
        utterance: "我想给爸妈换一台扫地机器人",
        fact: "用户想给爸妈换一台扫地机器人",
        expect: "扫地机器人",
        literal: "扫地机器人给爸妈买",
        colloquial: "上次说的扫地机器人买了吗",
        synonym: "给父母买自动清洁的家电",
    },
    Topic {
        utterance: "我不太喜欢开会，更愿意写文档",
        fact: "用户不太喜欢开会，更愿意写文档",
        expect: "开会",
        literal: "不喜欢开会更愿意写文档",
        colloquial: "之前说的不喜欢开会对吧",
        synonym: "更倾向书面沟通而不是当面讨论",
    },
];

/// 纯指代/半指代问法。前两条只含语气词与代词（切词后无腿），后两条带
/// 「之前/上次 + 那件事」（S1 之后脚手架词一并剥掉，也变成无腿）——
/// 四条的期望证据词与对话原文都取自 `TOPICS` 对应下标。
const ANAPHORA: [(&str, usize); 4] = [
    ("还记得吗", 11),
    ("那个呢", 10),
    ("上次那个事怎么样了", 9),
    ("之前提过的那件事", 3),
];

/// 久远事实（S2 新增的第六类）：事实只存在 90 天前，默认窗口（最近 7 天）
/// 里根本没有它。这类问法唯一可能的通路是索引跨时间召回——`beyond` 腿
/// 上线前它是 0%，上线后应当基本全中，这就是 S2 的收益量尺。
/// (问法, 事实原文, 期望证据词)
const LONG_AGO: [(&str, &str, &str); 4] = [
    ("我对花生过敏吗", "用户对花生过敏，外出点餐要避开", "花生过敏"),
    ("我的车牌号后四位是什么", "用户车牌尾号是 8823", "8823"),
    ("我用的什么显示器", "用户用 27 寸 4K 显示器，外接一台", "显示器"),
    ("我家猫叫什么名字", "用户养的猫叫团子，三岁", "团子"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// 问题本身就是原文片段——链路能力上限。
    ExactSlice,
    /// 自然句里裹着原文词——隔离切词损失。
    Literal,
    /// 「上次说的X…」——切词 + 脚手架词残留。
    Colloquial,
    /// 与原文无公共子串——纯语义缺口。
    Synonym,
    /// 只剩指代——窗口兜底与事实行上限。
    Anaphora,
    /// 事实只在 90 天前——默认窗口之外，只有索引跨时间召回能救。
    LongAgo,
}

impl Class {
    pub const ALL: [Class; 6] = [
        Class::ExactSlice,
        Class::Literal,
        Class::Colloquial,
        Class::Synonym,
        Class::Anaphora,
        Class::LongAgo,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Class::ExactSlice => "精确片段",
            Class::Literal => "字面自然句",
            Class::Colloquial => "口语指代句",
            Class::Synonym => "近义改写",
            Class::Anaphora => "纯指代",
            Class::LongAgo => "久远事实",
        }
    }
}

/// 一个用例跑完的结果。`pure_leg` 是指标 A，`hit_*` 是指标 B 的两个来源，
/// `degraded` 记录这次是不是走了「腿全落空 → 整窗兜底」。
#[derive(Debug)]
pub struct Outcome {
    pub class: Class,
    pub question: &'static str,
    pub legs: Vec<String>,
    pub pure_leg: bool,
    pub hit_lines: bool,
    pub hit_facts: bool,
    /// S2：命中来自窗口外的 `beyond` 腿（跨时间索引召回）。
    pub hit_beyond: bool,
    pub degraded: bool,
}

impl Outcome {
    pub fn hit(&self) -> bool {
        self.hit_lines || self.hit_facts || self.hit_beyond
    }

    fn source(&self) -> &'static str {
        match (self.hit_lines, self.hit_facts, self.hit_beyond) {
            (false, false, true) => "仅窗口外(索引)",
            _ if self.hit_lines && self.hit_facts => "对话+事实",
            _ if self.hit_lines => "仅对话行",
            _ if self.hit_facts => "仅事实行",
            _ => "未命中",
        }
    }
}

/// 单类汇总。
#[derive(Debug)]
pub struct ClassScore {
    pub class: Class,
    pub total: usize,
    pub pure: usize,
    pub hits: usize,
    /// 其中走了整窗兜底的条数——「命中」里有多少是碰巧撞进窗口。
    pub degraded: usize,
}

impl ClassScore {
    pub fn pure_rate(&self) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            self.pure as f64 / self.total as f64
        }
    }
    pub fn hit_rate(&self) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            self.hits as f64 / self.total as f64
        }
    }
    pub fn degraded_rate(&self) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            self.degraded as f64 / self.total as f64
        }
    }
}

pub struct Report {
    pub outcomes: Vec<Outcome>,
    pub scores: Vec<ClassScore>,
}

impl Report {
    pub fn score(&self, class: Class) -> &ClassScore {
        self.scores
            .iter()
            .find(|s| s.class == class)
            .expect("每个类别都应有汇总行")
    }

    /// 人读的表格 + 失败明细（`--nocapture` 看）。
    pub fn render(&self) -> String {
        let mut out = String::new();
        out.push_str("S0/S1/S2 检索基线 · memory_search 端到端（pinned 2026-12-01，语料见 seed）\n");
        out.push_str(&format!(
            "{}{}{}{}{}\n",
            pad("问法", 14),
            pad("用例", 6),
            pad("腿纯净", 8),
            pad("命中", 6),
            pad("走兜底", 8)
        ));
        for s in &self.scores {
            out.push_str(&format!(
                "{}{}{}{}{}\n",
                pad(s.class.label(), 14),
                pad(&s.total.to_string(), 6),
                pad(&format!("{:.0}%", s.pure_rate() * 100.0), 8),
                pad(&format!("{:.0}%", s.hit_rate() * 100.0), 6),
                pad(&format!("{:.0}%", s.degraded_rate() * 100.0), 8)
            ));
        }
        out.push_str("\n未命中用例（腿 → 结果来源）：\n");
        for o in self.outcomes.iter().filter(|o| !o.hit()) {
            out.push_str(&format!(
                "  [{}] {}  腿={:?}\n",
                o.class.label(),
                o.question,
                o.legs
            ));
        }
        out.push_str("\n命中来源分布：\n");
        for o in self.outcomes.iter().filter(|o| o.hit()) {
            out.push_str(&format!(
                "  [{}] {} → {}\n",
                o.class.label(),
                o.question,
                o.source()
            ));
        }
        out
    }
}

/// 中文按 2 列宽对齐（报告是给人读的，`{:<14}` 对不齐 CJK）。
fn pad(s: &str, width: usize) -> String {
    let w: usize = s.chars().map(|c| if c.is_ascii() { 1 } else { 2 }).sum();
    format!("{s}{}", " ".repeat(width.saturating_sub(w)))
}

/// 语料时间轴（相对 pinned 2026-12-01T02:00:00Z）：话题放三天前，噪声放
/// 一小时前。噪声必须是**更新**的那批，否则「腿全落空 → 整窗兜底」会
/// 白捡命中（S1 刚上线时基线就这么虚高过一轮：整窗把 12 条全捞回来，
/// 近义改写凭空 92%）。
const TOPIC_EVENT_TS: &str = "2026-11-28T02:00:00+00:00";
const TOPIC_FACT_TS: &str = "2026-11-28T03:00:00+00:00";
const NOISE_EVENT_TS: &str = "2026-12-01T01:00:00+00:00";
/// 噪声对话条数 = `MAX_DIALOGUE_ROWS`（60）。一周的真实语速是几百条，
/// 60 是「半天的量」——正好把上限顶满，让整窗兜底不再是免费午餐。
const NOISE_ROWS: usize = 60;
/// 噪声事实 40 条（≈ 一周 5–8 条/天）。`MAX_FACT_ROWS = 10`，所以整窗兜底
/// 只能带回最近 10 条噪声——「把整个窗口倒出来」在真实事实密度下并不成立。
const NOISE_FACTS: usize = 40;
/// 久远事实的时间戳：90 天前，稳稳在「最近一周」窗口之外。
const LONG_AGO_TS: &str = "2026-09-01T03:00:00+00:00";

/// 建库 + 播种：每个话题一条用户对话（原文）+ 一条事实（consolidate 的
/// 产物），再压 60 条更晚的日常噪声。
pub fn seed(conn: &Connection) -> rusqlite::Result<()> {
    db::init_schema_for_test(conn)?;
    for t in TOPICS.iter() {
        conn.execute(
            "INSERT INTO events (ts, kind, content, role, session_id)
             VALUES (?1, 'dialogue', ?2, 'user', 's0')",
            rusqlite::params![TOPIC_EVENT_TS, t.utterance],
        )?;
        conn.execute(
            "INSERT INTO facts (ts, content, confidence) VALUES (?1, ?2, 0.7)",
            rusqlite::params![TOPIC_FACT_TS, t.fact],
        )?;
    }
    for i in 0..NOISE_ROWS {
        conn.execute(
            "INSERT INTO events (ts, kind, content, role, session_id)
             VALUES (?1, 'dialogue', ?2, ?3, 'noise')",
            rusqlite::params![
                NOISE_EVENT_TS,
                format!("日常占位第{i}条"),
                if i % 2 == 0 { "user" } else { "assistant" }
            ],
        )?;
    }
    for i in 0..NOISE_FACTS {
        conn.execute(
            "INSERT INTO facts (ts, content, confidence) VALUES (?1, ?2, 0.6)",
            rusqlite::params![NOISE_EVENT_TS, format!("日常占位事实第{i}条")],
        )?;
    }
    // 久远事实：只存在 90 天前（默认窗口之外）。`beyond` 腿没上线时，
    // 这四条一个字都搜不到——这就是 S2 的收益量尺。
    for (_, fact, _) in LONG_AGO {
        conn.execute(
            "INSERT INTO facts (ts, content, confidence) VALUES (?1, ?2, 0.8)",
            rusqlite::params![LONG_AGO_TS, fact],
        )?;
    }
    Ok(())
}

/// 跑一遍全部用例。调用方负责先 pin 时钟（`clock::pinned`）。
pub fn run(conn: &Connection) -> Report {
    let mut outcomes = Vec::new();
    for (i, t) in TOPICS.iter().enumerate() {
        let cases = [
            (Class::ExactSlice, t.expect),
            (Class::Literal, t.literal),
            (Class::Colloquial, t.colloquial),
            (Class::Synonym, t.synonym),
        ];
        for (class, question) in cases {
            outcomes.push(measure(conn, class, question, i));
        }
    }
    for (question, topic) in ANAPHORA {
        outcomes.push(measure(conn, Class::Anaphora, question, topic));
    }
    for (question, fact, expect) in LONG_AGO {
        outcomes.push(measure_against(conn, Class::LongAgo, question, expect, fact));
    }

    let scores = Class::ALL
        .iter()
        .map(|class| {
            let rows: Vec<&Outcome> = outcomes.iter().filter(|o| o.class == *class).collect();
            ClassScore {
                class: *class,
                total: rows.len(),
                pure: rows.iter().filter(|o| o.pure_leg).count(),
                hits: rows.iter().filter(|o| o.hit()).count(),
                degraded: rows.iter().filter(|o| o.degraded).count(),
            }
        })
        .collect();
    Report { outcomes, scores }
}

/// 单例测量：切词（线上同一份实现）→ 腿纯净判定 → 端到端召回。
/// 腿纯净只看**目标话题**的原文——拿别的话题算命中会把分数哄高。
fn measure(conn: &Connection, class: Class, question: &'static str, topic: usize) -> Outcome {
    let target = &TOPICS[topic];
    measure_against(conn, class, question, target.expect, target.utterance)
}

/// 通用版：`expect` 是要在召回结果里看到的证据词，`doc` 是腿纯净判定的
/// 比对原文（久远事实类没有「对话原文」，用事实原文本身）。
fn measure_against(
    conn: &Connection,
    class: Class,
    question: &'static str,
    expect: &str,
    doc: &str,
) -> Outcome {
    let today = super::clock::now_local().date_naive();
    let phrase = memory_search::parse_window(question, today)
        .map(|(_, _, label)| label)
        .unwrap_or_default();
    let legs = memory_search::legs(question, &phrase);
    let pure_leg = legs.iter().any(|k| doc.contains(k.as_str()));

    let recall = memory_search::recall(conn, question);
    let hit_lines = recall
        .lines
        .iter()
        .any(|(_, _, text)| text.contains(expect));
    let hit_facts = recall.facts.iter().any(|f| f.contains(expect));
    let hit_beyond = recall.beyond.iter().any(|(_, text)| text.contains(expect));

    Outcome {
        class,
        question,
        legs,
        pure_leg,
        hit_lines,
        hit_facts,
        hit_beyond,
        degraded: recall.degraded,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::clock;
    use rusqlite::Connection;

    const NOW: &str = "2026-12-01T02:00:00Z"; // 本地 12-01 10:00

    fn bench() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        seed(&conn).unwrap();
        conn
    }

    /// S0 交付物：一张基线表。门槛只做「不许比基线更差」的棘轮——
    /// S1/S2 上线后把 FLOOR 上调，embedding 的触发条件才有数可依。
    #[test]
    fn recall_baseline_by_question_class() {
        let _g = clock::pinned(NOW);
        let conn = bench();
        let report = run(&conn);
        println!("{}", report.render());

        // 结构自检：每个用例都必须落到某一类里，总数对得上。
        assert_eq!(
            report.outcomes.len(),
            TOPICS.len() * 4 + ANAPHORA.len() + LONG_AGO.len(),
            "用例总数"
        );
        assert_eq!(
            report.scores.iter().map(|s| s.total).sum::<usize>(),
            report.outcomes.len(),
            "分类汇总必须覆盖全部用例"
        );

        // 能力上限：精确片段必须基本全中——不中说明链路本身坏了，
        // 与切词/语义无关，要单独报警。
        assert!(
            report.score(Class::ExactSlice).hit_rate() >= 0.9,
            "精确片段命中率跌破 90%，链路本身退化：{}",
            report.render()
        );
    }

    /// 记录型断言（不设质量门槛）：把当前各类命中数钉住，任何改动让
    /// 数字变差都是可见的回归，而不是「感觉好像变笨了」。
    #[test]
    fn baseline_numbers_are_ratcheted() {
        let _g = clock::pinned(NOW);
        let conn = bench();
        let report = run(&conn);

        let hits: Vec<(Class, usize)> = Class::ALL
            .iter()
            .map(|c| (*c, report.score(*c).hits))
            .collect();
        // 2026-11-24 S3：近义 12/12。2026-11-24 R3（plan-reliability-phase1）：
        // 拆除对立/因果/上下位假同义后，花生过敏条近义问法不再靠错误扩腿命中，
        // Synonym 地板改为 11（只许因正确性修复下调这一次，并写明原因）。
        let floor: [(Class, usize); 6] = [
            (Class::ExactSlice, 12),
            (Class::Literal, 12),
            (Class::Colloquial, 12),
            (Class::Synonym, 11),
            (Class::Anaphora, 0),
            (Class::LongAgo, 4),
        ];
        for ((class, got), (_, need)) in hits.iter().zip(floor.iter()) {
            assert!(*got >= *need, "{:?} 命中数从基线 {need} 掉到 {got}", class);
        }
    }
}

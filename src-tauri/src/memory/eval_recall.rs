//! S0 检索基线（2026-11-23）：向量检索决策的量化前置。
//!
//! 决策背景（对 C3「记忆与个性化」的核查）：`memory_search` 现在只有
//! 「日期窗口 + 关键词子串」一条腿，`db.rs:358` 的向量 TODO 与 roadmap
//! G-13 / P5-1 都写着「等触发条件」。要判断 embedding 值不值，先得把
//! 「现在到底丢在哪一步」量成数字——这份基线就是这个数字。
//!
//! 五类问法，两个指标：
//!
//! | 类别 | 问题形态 | 用来隔离哪一段 |
//! | --- | --- | --- |
//! | 精确片段 | 问题本身就是存过的原文片段 | 链路本身能不能跑通（能力上限） |
//! | 字面自然句 | 含原文词，但裹在自然句里 | 切词（`keywords_with`）损失 |
//! | 口语指代句 | 「上次说的X…」 | 切词 + 脚手架词残留 |
//! | 近义改写 | 与原文无公共子串 | 纯语义缺口（只有 embedding 能救的量） |
//! | 纯指代 | 「还记得吗」 | 窗口兜底 + 事实行上限 |
//!
//! 指标 A「腿纯净率」= 抽出的关键词腿里至少有一条是库里真实子串的比例。
//! 指标 B「端到端命中率」= 模型最终能在召回结果里看到该主题的比例
//! （对话行或事实行任一命中即算——模型看到的是两者）。
//!
//! 指标 A 与 B 的差就是**实现缺陷可回收的收益**；近义改写那行的 B 就是
//! **只有 embedding 才能回收的收益**。S1（三级降级）/ S2（FTS5 trigram）
//! 上线后重跑本文件，B 应显著上移而「近义改写」行不动——那才是引入向量
//! 的正当理由。
//!
//! 首次基线（2026-11-23，12 事实 / 12 对话 / 固定时钟 2026-12-01）：
//!
//! ```text
//! 问法          用例  腿纯净  端到端命中
//! 精确片段      12    92%     100%
//! 字面自然句    12    8%      8%
//! 口语指代句    12    33%     33%
//! 近义改写      12    0%      0%
//! 纯指代        4     0%      50%
//! ```
//!
//! 三个可直接归因的结论：
//! 1. **链路本身是好的**（精确片段 100%），丢的全在切词：自然句只在「整段
//!    残句恰好是原文子串」时才命中（唯一成功的是「花生过敏」，因为前面的
//!    字全是停用字）。
//! 2. **停用字会把词切碎**：「所有权」里的「有」是停用字 → 切成「所 / 权」
//!    两段单字被丢弃 → 一条腿都没有（精确片段 92% 而非 100% 就是它）。
//! 3. **窗口兜底是有效的，但被垃圾腿挡住**：纯指代 50%——「还记得吗 / 那个呢」
//!    无腿 → 整窗扫描 → 靠对话行命中；「上次那个事怎么样了」抽出 `["上次"]`、
//!    「之前提过的那件事」抽出 `["之前","件事"]` → `AND (LIKE)` 归零 →
//!    报「没有找到记录」。**零命中不降级**是这里唯一的死因。
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
/// 12 条事实 > `MAX_FACT_ROWS = 10`，故意越过事实行上限一档——真实使用下
/// 每天 5–15 条事实，一周窗口本来就超 10 条。
const TOPICS: [Topic; 12] = [
    Topic {
        utterance: "我在走报销流程，先找直属主管审批再去财务系统",
        fact: "用户在走报销流程，先找主管审批再进财务系统",
        expect: "报销流程",
        literal: "报销流程怎么走",
        colloquial: "上次说的报销流程到哪一步了",
        synonym: "那个退钱的手续还没弄完",
    },
    Topic {
        utterance: "我家客厅阳台想封窗，尺寸已经量过了",
        fact: "用户家客厅阳台准备封窗，尺寸已量",
        expect: "封窗",
        literal: "封窗尺寸量了吗",
        colloquial: "之前那个封窗的事有进展吗",
        synonym: "阳台装窗户的测量结果",
    },
    Topic {
        utterance: "我喜欢雨天跑步，一般晚上八点出门",
        fact: "用户喜欢雨天跑步，晚上八点出门",
        expect: "雨天跑步",
        literal: "雨天跑步几点出门",
        colloquial: "上次聊的那个雨天跑步几点来着",
        synonym: "下雨天夜跑的时间安排",
    },
    Topic {
        utterance: "我在杭州余杭看二手房，预算三百万左右",
        fact: "用户在杭州余杭看二手房，预算三百万",
        expect: "二手房",
        literal: "二手房预算多少",
        colloquial: "之前那个二手房的预算还记得吗",
        synonym: "看中的房子总价上限",
    },
    Topic {
        utterance: "周末想和女朋友自驾去舟山",
        fact: "用户周末想和女朋友自驾去舟山",
        expect: "自驾去舟山",
        literal: "自驾去舟山要多久",
        colloquial: "上次说的自驾去舟山还去吗",
        synonym: "开车带对象去海边玩",
    },
    Topic {
        utterance: "我在做一个复盘小程序，用毛泽东方法论",
        fact: "用户在做一个复盘小程序，用毛泽东方法论",
        expect: "复盘小程序",
        literal: "复盘小程序用什么方法论",
        colloquial: "那个复盘小程序做到哪了",
        synonym: "毛选式反思那个应用进度",
    },
    Topic {
        utterance: "我对花生过敏，点菜要避开",
        fact: "用户对花生过敏，点菜要避开",
        expect: "花生过敏",
        literal: "花生过敏点菜注意什么",
        colloquial: "上次说的花生过敏还记得吧",
        synonym: "坚果类不能吃，外食要提醒",
    },
    Topic {
        utterance: "我每天早上喝一杯黑咖啡，不加糖",
        fact: "用户每天早上喝一杯黑咖啡，不加糖",
        expect: "黑咖啡",
        literal: "黑咖啡加不加糖",
        colloquial: "之前说的黑咖啡怎么喝来着",
        synonym: "早晨那杯纯咖啡不放糖",
    },
    Topic {
        utterance: "我家猫叫团子，今年三岁",
        fact: "用户的猫叫团子，今年三岁",
        expect: "团子",
        literal: "猫叫团子几岁",
        colloquial: "上次说的猫叫团子多大了",
        synonym: "家里那只宠物猫的年龄",
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
/// 「之前/上次 + 那件事」（切词后留下不可用的残片）——两类对照，正好分离
/// 「窗口兜底没做」与「垃圾腿挡住兜底」两种失败。
/// (问法, 期望证据词, 该话题在 TOPICS 里的下标)
const ANAPHORA: [(&str, &str, usize); 4] = [
    ("还记得吗", "开会", 11),
    ("那个呢", "扫地机器人", 10),
    ("上次那个事怎么样了", "所有权", 9),
    ("之前提过的那件事", "二手房", 3),
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
}

impl Class {
    pub const ALL: [Class; 5] = [
        Class::ExactSlice,
        Class::Literal,
        Class::Colloquial,
        Class::Synonym,
        Class::Anaphora,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Class::ExactSlice => "精确片段",
            Class::Literal => "字面自然句",
            Class::Colloquial => "口语指代句",
            Class::Synonym => "近义改写",
            Class::Anaphora => "纯指代",
        }
    }
}

/// 一个用例跑完的结果。`pure_leg` 是指标 A，`hit_*` 是指标 B 的两个来源。
#[derive(Debug)]
pub struct Outcome {
    pub class: Class,
    pub question: &'static str,
    pub legs: Vec<String>,
    pub pure_leg: bool,
    pub hit_lines: bool,
    pub hit_facts: bool,
}

impl Outcome {
    pub fn hit(&self) -> bool {
        self.hit_lines || self.hit_facts
    }

    fn source(&self) -> &'static str {
        match (self.hit_lines, self.hit_facts) {
            (true, true) => "对话+事实",
            (true, false) => "仅对话行",
            (false, true) => "仅事实行",
            (false, false) => "未命中",
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
}

impl ClassScore {
    pub fn pure_rate(&self) -> f64 {
        if self.total == 0 { 0.0 } else { self.pure as f64 / self.total as f64 }
    }
    pub fn hit_rate(&self) -> f64 {
        if self.total == 0 { 0.0 } else { self.hits as f64 / self.total as f64 }
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
        out.push_str("S0 检索基线 · memory_search 端到端（pinned 2026-12-01）\n");
        out.push_str(&format!(
            "{}{}{}{}\n",
            pad("问法", 14),
            pad("用例", 6),
            pad("腿纯净", 8),
            pad("端到端命中", 10)
        ));
        for s in &self.scores {
            out.push_str(&format!(
                "{}{}{}{}\n",
                pad(s.class.label(), 14),
                pad(&s.total.to_string(), 6),
                pad(&format!("{:.0}%", s.pure_rate() * 100.0), 8),
                pad(&format!("{:.0}%", s.hit_rate() * 100.0), 10)
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
            out.push_str(&format!("  [{}] {} → {}\n", o.class.label(), o.question, o.source()));
        }
        out
    }
}

/// 中文按 2 列宽对齐（报告是给人读的，`{:<14}` 对不齐 CJK）。
fn pad(s: &str, width: usize) -> String {
    let w: usize = s.chars().map(|c| if c.is_ascii() { 1 } else { 2 }).sum();
    format!("{s}{}", " ".repeat(width.saturating_sub(w)))
}

/// 建库 + 播种：每个话题一条用户对话（原文）+ 一条事实（consolidate 的产物）。
pub fn seed(conn: &Connection) -> rusqlite::Result<()> {
    db::init_schema_for_test(conn)?;
    for t in TOPICS.iter() {
        db::add_event_meta(conn, "dialogue", t.utterance, Some("user"), Some("s0"))?;
        db::add_fact_confident(conn, t.fact, None, 0.7)?;
    }
    Ok(())
}

/// 跑一遍全部用例。调用方负责先 pin 时钟（`clock::pinned`）。
pub fn run(conn: &Connection) -> Report {
    let mut outcomes = Vec::new();
    for t in TOPICS.iter() {
        let cases = [
            (Class::ExactSlice, t.expect),
            (Class::Literal, t.literal),
            (Class::Colloquial, t.colloquial),
            (Class::Synonym, t.synonym),
        ];
        for (class, question) in cases {
            outcomes.push(measure(conn, class, question, t.expect));
        }
    }
    for (question, expect, _topic) in ANAPHORA {
        outcomes.push(measure(conn, Class::Anaphora, question, expect));
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
            }
        })
        .collect();
    Report { outcomes, scores }
}

/// 单例测量：切词（线上同一份实现）→ 腿纯净判定 → 端到端召回。
fn measure(conn: &Connection, class: Class, question: &'static str, expect: &str) -> Outcome {
    let today = super::clock::now_local().date_naive();
    let phrase = memory_search::parse_window(question, today)
        .map(|(_, _, label)| label)
        .unwrap_or_default();
    let legs = memory_search::keywords(question, &phrase);
    // 腿纯净 = 至少一条腿是库里真实存在的子串（LIKE 能命中）。
    let pure_leg = legs
        .iter()
        .any(|k| TOPICS.iter().any(|t| t.utterance.contains(k.as_str())));

    let recall = memory_search::recall(conn, question);
    let hit_lines = recall.lines.iter().any(|(_, _, text)| text.contains(expect));
    let hit_facts = recall.facts.iter().any(|f| f.contains(expect));

    Outcome { class, question, legs, pure_leg, hit_lines, hit_facts }
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
            TOPICS.len() * 4 + ANAPHORA.len(),
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

        let hits: Vec<(Class, usize)> =
            Class::ALL.iter().map(|c| (*c, report.score(*c).hits)).collect();
        // 2026-11-23 首次测得的命中数（用例数 12/12/12/12/4）。S1/S2 或
        // embedding 上线后上调，永不为了「让测试过」而下调。
        // 「近义改写」是 0——这一行刻意留 0，它就是 embedding 的靶子：
        // 只有它上移，才说明向量腿真的换来了东西。
        let floor: [(Class, usize); 5] = [
            (Class::ExactSlice, 12),
            (Class::Literal, 1),
            (Class::Colloquial, 4),
            (Class::Synonym, 0),
            (Class::Anaphora, 2),
        ];
        for ((class, got), (_, need)) in hits.iter().zip(floor.iter()) {
            assert!(
                *got >= *need,
                "{:?} 命中数从基线 {need} 掉到 {got}",
                class
            );
        }
    }
}

//! 统一规则意图匹配核（优化盘点 P2-9）。
//!
//! 日历 / 总结 / 规划 / 取消曾各自手搓「排除词 + 双命中 + 宁漏勿误」。
//! 本模块只提供**表驱动匹配原语**；领域标签与管线顺序仍在调用方
//! （危机 → 日历 → 总结 → 模型/规划），不在此做总调度。
//!
//! 行为契约：既有 20 正 / 15 负（及日历/总结同类）CI 样本必须逐字等价。

/// 去空白 + 小写。规则路由的统一预处理；捕获类规则在归一化后的串上跑。
pub fn normalise(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_lowercase()
}

/// 一条匹配条件。领域表组合这些原语，避免再复制 contains 循环。
#[derive(Debug, Clone, Copy)]
pub enum Match<'a> {
    /// 任一短语命中（长短语表、强 cue）。
    Any(&'a [&'a str]),
    /// 每组至少命中一词（双命中：时间词 ∩ 日程名词、动词 ∩ 窗口词）。
    All(&'a [&'a [&'a str]]),
    /// 词表中 ≥ `min` 个**不同**条目命中（多动作 / 多域 / 成对序列词）。
    AtLeast { lexicon: &'a [&'a str], min: usize },
    /// 整句精确等于其一（短取消指令）。
    Exact(&'a [&'a str]),
    /// 含短语且整句字节长 ≤ 短语长 + `slack`（短取消，防「别做了那件事…」）。
    NearPhrase { phrases: &'a [&'a str], slack: usize },
}

impl Match<'_> {
    pub fn hits(&self, hay: &str) -> bool {
        match *self {
            Match::Any(phrases) => phrases.iter().any(|p| hay.contains(p)),
            Match::All(groups) => groups
                .iter()
                .all(|g| g.iter().any(|p| hay.contains(p))),
            Match::AtLeast { lexicon, min } => {
                lexicon.iter().filter(|p| hay.contains(**p)).count() >= min
            }
            Match::Exact(phrases) => phrases.iter().any(|p| hay == *p),
            Match::NearPhrase { phrases, slack } => phrases.iter().any(|s| {
                hay == *s || (hay.len() <= s.len() + slack && hay.contains(s))
            }),
        }
    }
}

/// 有序规则：先匹配先生效。
#[derive(Debug, Clone, Copy)]
pub struct Rule<'a, L> {
    pub when: Match<'a>,
    pub label: L,
}

/// 排除词 + 有序规则表。命中排除 → `None`；否则返回第一条命中规则的标签。
#[derive(Debug, Clone, Copy)]
pub struct Table<'a, L> {
    pub exclusions: &'a [&'a str],
    pub rules: &'a [Rule<'a, L>],
}

impl<'a, L: Clone> Table<'a, L> {
    /// 对已归一化文本跑表。空串 / 排除 → `None`。
    pub fn first_on_normalised(&self, t: &str) -> Option<L> {
        if t.is_empty() {
            return None;
        }
        if self.exclusions.iter().any(|w| t.contains(w)) {
            return None;
        }
        for r in self.rules {
            if r.when.hits(t) {
                return Some(r.label.clone());
            }
        }
        None
    }

    /// 归一化后跑表。
    pub fn first(&self, text: &str) -> Option<L> {
        self.first_on_normalised(&normalise(text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalise_strips_whitespace_and_lowercases() {
        assert_eq!(normalise("  Hello 世界\t"), "hello世界");
    }

    #[test]
    fn any_and_all_and_at_least() {
        assert!(Match::Any(&["日程", "安排"]).hits("今天日程"));
        assert!(!Match::Any(&["日程"]).hits("今天好累"));
        assert!(Match::All(&[&["今天", "今日"], &["总结", "回顾"]]).hits("今日回顾"));
        assert!(!Match::All(&[&["今天"], &["总结"]]).hits("今天好累"));
        assert!(Match::AtLeast {
            lexicon: &["先", "再", "然后"],
            min: 2
        }
        .hits("先查再写"));
        assert!(!Match::AtLeast {
            lexicon: &["先", "再"],
            min: 2
        }
        .hits("先这样"));
    }

    #[test]
    fn exact_and_near_phrase_cancel_shape() {
        assert!(Match::Exact(&["停", "取消"]).hits("停"));
        assert!(!Match::Exact(&["停"]).hits("停一下"));
        let near = Match::NearPhrase {
            phrases: &["别做了", "停一下"],
            slack: 2,
        };
        assert!(near.hits("别做了"));
        assert!(near.hits("停一下"));
        assert!(!near.hits("别做了那件事吧先放着"));
    }

    #[test]
    fn table_exclusions_and_first_match_win() {
        #[derive(Clone, Copy, Debug, PartialEq)]
        enum L {
            A,
            B,
        }
        const TABLE: Table<'static, L> = Table {
            exclusions: &["纪要"],
            rules: &[
                Rule {
                    when: Match::Any(&["周报"]),
                    label: L::B,
                },
                Rule {
                    when: Match::Any(&["日报"]),
                    label: L::A,
                },
            ],
        };
        assert_eq!(TABLE.first("周报"), Some(L::B));
        assert_eq!(TABLE.first("日报"), Some(L::A));
        assert_eq!(TABLE.first("会议纪要日报"), None);
        assert_eq!(TABLE.first(""), None);
    }
}

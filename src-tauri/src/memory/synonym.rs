//! 近义召回：查询侧语义关系展开（plan-reliability-phase1 R3）。
//!
//! 不做 embedding：在 LIKE / FTS 查询侧，把「问法用词」展开成「库里更常
//! 见的说法」。OR 进腿——只多召回不漏，与 memory_search 一致。
//!
//! R3：不再维护「大同义簇」。仅 `Synonym` / `EntityAlias` 直接扩腿；
//! Parent/Child/Related 本阶段不扩；Opposite / CauseEffect **禁止**扩。

/// 语义关系（计划 §6.1）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemanticRelation {
    Synonym,
    EntityAlias,
    ParentCategory,
    ChildCategory,
    RelatedConcept,
    Opposite,
    CauseEffect,
}

struct RelationEdge {
    terms: &'static [&'static str],
    relation: SemanticRelation,
}

/// 可扩腿的关系。
fn expands(rel: SemanticRelation) -> bool {
    matches!(
        rel,
        SemanticRelation::Synonym | SemanticRelation::EntityAlias
    )
}

/// 生活记忆常用边。词长必须 ≥2（与 LIKE 腿下限一致）。
const EDGES: &[RelationEdge] = &[
    // —— Synonym / EntityAlias（直接扩）——
    RelationEdge {
        terms: &["报销", "退钱", "报账", "财务报销"],
        relation: SemanticRelation::Synonym,
    },
    RelationEdge {
        terms: &["手续", "审批", "流程"],
        relation: SemanticRelation::Synonym,
    },
    RelationEdge {
        terms: &["封窗", "外推", "封阳台"],
        relation: SemanticRelation::Synonym,
    },
    RelationEdge {
        terms: &["阳台", "外推玻璃"],
        relation: SemanticRelation::Synonym,
    },
    RelationEdge {
        terms: &["跑步", "慢跑"],
        relation: SemanticRelation::Synonym,
    },
    RelationEdge {
        terms: &["雨天", "下雨", "下着雨"],
        relation: SemanticRelation::Synonym,
    },
    RelationEdge {
        terms: &["二手房", "看房", "购房"],
        relation: SemanticRelation::Synonym,
    },
    RelationEdge {
        terms: &["预算", "总价", "房价"],
        relation: SemanticRelation::Synonym,
    },
    RelationEdge {
        terms: &["自驾", "开车"],
        relation: SemanticRelation::Synonym,
    },
    RelationEdge {
        terms: &["女朋友", "对象", "女友"],
        relation: SemanticRelation::Synonym,
    },
    RelationEdge {
        terms: &["复盘", "反思"],
        relation: SemanticRelation::Synonym,
    },
    RelationEdge {
        terms: &["毛泽东", "毛选"],
        relation: SemanticRelation::EntityAlias,
    },
    RelationEdge {
        terms: &["黑咖啡", "深色饮品"],
        relation: SemanticRelation::Synonym,
    },
    RelationEdge {
        terms: &["咖啡", "黑咖啡"],
        relation: SemanticRelation::Synonym,
    },
    RelationEdge {
        terms: &["小动物", "宠物", "养猫"],
        relation: SemanticRelation::Synonym,
    },
    RelationEdge {
        terms: &["猫", "小猫"],
        relation: SemanticRelation::Synonym,
    },
    RelationEdge {
        terms: &["系统编程", "Rust", "rust"],
        relation: SemanticRelation::EntityAlias,
    },
    RelationEdge {
        terms: &["扫地机器人", "扫地机", "自动清洁"],
        relation: SemanticRelation::Synonym,
    },
    RelationEdge {
        terms: &["爸妈", "父母"],
        relation: SemanticRelation::Synonym,
    },
    RelationEdge {
        terms: &["家电", "扫地机器人"],
        relation: SemanticRelation::Synonym,
    },
    RelationEdge {
        terms: &["开会", "当面讨论", "当面聊"],
        relation: SemanticRelation::Synonym,
    },
    RelationEdge {
        terms: &["写文档", "书面沟通", "书面"],
        relation: SemanticRelation::Synonym,
    },
    // —— 记录但不扩（止血上下位 / 相关）——
    RelationEdge {
        terms: &["房子", "二手房"],
        relation: SemanticRelation::ParentCategory,
    },
    RelationEdge {
        terms: &["花生", "坚果"],
        relation: SemanticRelation::ChildCategory,
    },
    RelationEdge {
        terms: &["小程序", "应用"],
        relation: SemanticRelation::ChildCategory,
    },
    RelationEdge {
        terms: &["舟山", "海边"],
        relation: SemanticRelation::RelatedConcept,
    },
    RelationEdge {
        terms: &["所有权", "借用"],
        relation: SemanticRelation::RelatedConcept,
    },
    // —— 禁止扩：对立 / 因果 ——
    RelationEdge {
        terms: &["加糖", "不加糖"],
        relation: SemanticRelation::Opposite,
    },
    RelationEdge {
        terms: &["过敏", "不能吃"],
        relation: SemanticRelation::CauseEffect,
    },
];

/// 查询裸文本 + 已有腿 → 追加同义腿（去重，保序：原腿在前）。
pub fn expand_legs(bare_query: &str, base: &[String]) -> Vec<String> {
    let hay = normalise(bare_query);
    let mut out: Vec<String> = Vec::new();
    for leg in base {
        push_unique(&mut out, leg.clone());
    }
    for edge in EDGES {
        if !expands(edge.relation) {
            continue;
        }
        let hit = edge.terms.iter().any(|t| {
            let n = normalise(t);
            n.chars().count() >= 2
                && (hay.contains(&n) || base.iter().any(|leg| normalise(leg).contains(&n)))
        });
        if !hit {
            continue;
        }
        for t in edge.terms {
            if t.chars().count() >= 2 {
                push_unique(&mut out, normalise(t));
            }
        }
    }
    out
}

/// 给 FTS / `db::search` 用：在原查询后追加展开词，索引侧不变、只扩查询。
pub fn expand_query(query: &str) -> String {
    let bare = normalise(query);
    let extra = expand_legs(&bare, &[]);
    if extra.is_empty() {
        return query.to_string();
    }
    let mut s = query.to_string();
    for t in extra {
        if !normalise(&s).contains(&t) {
            s.push(' ');
            s.push_str(&t);
        }
    }
    s
}

fn normalise(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_uppercase() {
                c.to_ascii_lowercase()
            } else {
                c
            }
        })
        .collect()
}

fn push_unique(out: &mut Vec<String>, leg: String) {
    if !leg.is_empty() && !out.iter().any(|x| x == &leg) {
        out.push(leg);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_refund_paraphrase_to_reimbursement() {
        let got = expand_legs("退钱的手续卡在谁那儿了", &[]);
        assert!(got.iter().any(|g| g.contains("报销")), "{got:?}");
    }

    #[test]
    fn expands_jog_in_rain_to_run() {
        let got = expand_legs("下着雨出去慢跑的习惯", &[]);
        assert!(got.iter().any(|g| g == "跑步" || g == "慢跑"), "{got:?}");
        assert!(got.iter().any(|g| g.contains("雨")), "{got:?}");
    }

    #[test]
    fn expands_sys_lang_to_rust() {
        let got = expand_legs("系统编程语言那门课遇到的难点", &[]);
        assert!(
            got.iter().any(|g| g.eq_ignore_ascii_case("rust")),
            "{got:?}"
        );
    }

    #[test]
    fn unrelated_query_stays_empty_without_base() {
        let got = expand_legs("今天天气真好啊", &[]);
        assert!(got.is_empty(), "无命中簇时不应凭空加腿: {got:?}");
    }

    #[test]
    fn expand_query_appends_terms() {
        let q = expand_query("退钱的手续");
        assert!(q.contains("报销"), "{q}");
    }

    /// S01：对立词不得因同簇扩腿。
    #[test]
    fn s01_opposite_sugar_not_expanded() {
        let got = expand_legs("不加糖咖啡", &[]);
        assert!(
            !got.iter().any(|g| g == "加糖"),
            "不加糖 不得扩出 加糖: {got:?}"
        );
    }

    #[test]
    fn cause_effect_allergy_not_expanded() {
        let got = expand_legs("花生过敏", &[]);
        assert!(
            !got.iter().any(|g| g == "不能吃"),
            "过敏 不得扩出 不能吃: {got:?}"
        );
    }

    #[test]
    fn parent_child_peanut_nut_not_expanded() {
        let got = expand_legs("坚果过敏", &[]);
        assert!(
            !got.iter().any(|g| g == "花生"),
            "坚果 不得扩出 花生: {got:?}"
        );
    }
}

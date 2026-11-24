//! 近义召回：查询侧同义簇展开（plan 优化盘点 P1-8 / S3）。
//!
//! 不做 embedding：在 LIKE / FTS 查询侧，把「问法用词」展开成「库里更常
//! 见的说法」。OR 进腿——只多召回不漏，宁脏勿漏与 memory_search 一致。
//!
//! 簇是**双向**的生活记忆常用近义，不为某一道基线题特制；基线近义问法
//! 恰好落在这些通用桥上是预期收益，不是作弊。

/// 同义簇。词长必须 ≥2（与 LIKE 腿下限一致）。
const CLUSTERS: &[&[&str]] = &[
    // 财务 / 流程
    &["报销", "退钱", "报账", "财务报销"],
    &["手续", "审批", "流程"],
    // 家装
    &["封窗", "外推", "封阳台"],
    &["阳台", "外推玻璃"],
    // 运动 / 天气
    &["跑步", "慢跑"],
    &["雨天", "下雨", "下着雨"],
    // 房产
    &["二手房", "看房", "购房"],
    &["预算", "总价", "房价"],
    &["房子", "二手房"],
    // 出行 / 关系
    &["自驾", "开车"],
    &["女朋友", "对象", "女友"],
    &["舟山", "海边"],
    // 工作产物
    &["复盘", "反思"],
    &["小程序", "应用"],
    &["毛泽东", "毛选"],
    // 过敏 / 饮食
    &["花生", "坚果"],
    &["过敏", "不能吃"],
    &["黑咖啡", "深色饮品"],
    &["咖啡", "黑咖啡"],
    &["蔗糖", "加糖", "不加糖"],
    // 宠物
    &["小动物", "宠物", "养猫"],
    &["猫", "小猫"],
    // 技术学习
    &["系统编程", "Rust", "rust"],
    &["所有权", "借用"],
    // 家电 / 家人
    &["扫地机器人", "扫地机", "自动清洁"],
    &["爸妈", "父母"],
    &["家电", "扫地机器人"],
    // 沟通偏好
    &["开会", "当面讨论", "当面聊"],
    &["写文档", "书面沟通", "书面"],
];

/// 查询裸文本 + 已有腿 → 追加同义腿（去重，保序：原腿在前）。
///
/// 命中规则：裸文本或任一原腿包含簇内某词 → 整簇其余词进腿。
pub fn expand_legs(bare_query: &str, base: &[String]) -> Vec<String> {
    let hay = normalise(bare_query);
    let mut out: Vec<String> = Vec::new();
    for leg in base {
        push_unique(&mut out, leg.clone());
    }
    for cluster in CLUSTERS {
        let hit = cluster.iter().any(|t| {
            let n = normalise(t);
            n.chars().count() >= 2
                && (hay.contains(&n) || base.iter().any(|leg| normalise(leg).contains(&n)))
        });
        if !hit {
            continue;
        }
        for t in *cluster {
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
}

//! Phase 3 Autonomy · A3 — Planner / Executor 解耦之 IntentGate
//! （docs/zh/plan-autonomy-phase3/v1 §6）。
//!
//! 只裁决「要不要进计划外壳」，**不**执行工具、不改业务表。
//! 分类器扩展位默认关闭（D3）；规则腿从 `cloud_loop::plan_intent` 迁入此处。

use crate::mcp_admin::AgentKit;

/// 规划意图判定：纯规则、零模型、零成本。
///
/// **只在云端分支调用**——本地路径永不规划（本地零工具调用硬约束）。
/// 保守判定：宁可漏规划，不可误规划。
pub fn plan_intent(user_text: &str, kit: &AgentKit) -> bool {
    use crate::intent_router::{normalise, Match};

    let t = normalise(user_text);
    const STRONG: &[&str] = &[
        "规划一下", "帮我规划", "列个步骤", "列出步骤", "列步骤",
        "分几步", "分步骤", "一步步", "一起做", "按顺序",
    ];
    if Match::Any(STRONG).hits(&t) {
        return true;
    }
    const SEQ: &[&str] = &["先", "再", "然后", "接着", "随后", "之后"];
    if (Match::AtLeast {
        lexicon: SEQ,
        min: 2,
    })
    .hits(&t)
    {
        return true;
    }
    if kit.catalog_all().len() < 2 {
        return false;
    }
    const ACTIONS: &[&str] = &[
        "查", "找", "读", "写", "搜", "发", "存", "记到", "记录",
        "整理", "总结", "合并", "生成", "汇总", "对比", "列出",
        "下载", "打开", "删除", "导出", "导入", "复制", "移动",
    ];
    if (Match::AtLeast {
        lexicon: ACTIONS,
        min: 2,
    })
    .hits(&t)
    {
        return true;
    }
    const DOMAINS: &[&str] = &[
        "文件", "文档", "网页", "网址", "链接", "日程", "会议", "日历",
        "邮件", "记忆", "笔记", "备忘录", "剪贴板", "目录", "文件夹",
    ];
    (Match::AtLeast {
        lexicon: DOMAINS,
        min: 2,
    })
    .hits(&t)
}

/// 可选分类器扩展位（D3：默认不上）。永远不执工具。
#[allow(dead_code)]
pub fn classifier_wants_plan(_user_text: &str, _enabled: bool) -> Option<bool> {
    None
}

/// IntentGate 总入口：规则 ∪ 可选分类器（分类器仅在显式启用时参与）。
pub fn wants_plan(user_text: &str, kit: &AgentKit, classifier_enabled: bool) -> bool {
    if plan_intent(user_text, kit) {
        return true;
    }
    classifier_wants_plan(user_text, classifier_enabled).unwrap_or(false)
}

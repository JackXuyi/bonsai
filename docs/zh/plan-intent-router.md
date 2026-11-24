# plan-intent-router — 统一规则意图匹配核

| 文档信息 | 内容 |
|---|---|
| 版本 | v0.1（已实施） |
| 日期 | 2026-11-24 |
| 状态 | **已实施**（T1–T3） |
| 上游输入 | 优化盘点 P2-9；`plan-calendar` §3.5、`plan-summary-skill`、`plan-planning` §5.1、`plan-retry` 取消意图 |
| 定位约束 | 宁漏勿误；正负样本 CI 不变；能力不重复实现；本地零工具调用 |
| 一句话 | 把「排除词 + 双命中 + 词表计数」收成一份匹配核，领域只留词表与标签 |

---

## 0. 结论

日历 `tools::route`、总结 `summary::intent`、规划 `plan_intent`、取消 `cancel_intent` 各自复制同一套手搓循环。抽 `intent_router`：**匹配原语一份权威**；领域标签与管线顺序仍在调用方。

### 取舍

| # | 裁决 | 理由 |
|---|---|---|
| D1 | 不做总调度器 | 危机→日历→总结→模型顺序是管线职责，合并会抢意图 |
| D2 | 领域保留自有标签类型 | `ToolIntent` / `SummaryKind` / `bool` 不揉成巨型枚举 |
| D3 | Search / kit 门控走表外 | 查询捕获与 `kit.len()` 不是纯标签表能表达的 |
| D4 | `might_recall` 不进 V1 | 闸门语义不同，另案 |

---

## 1. 形状

```text
intent_router::{normalise, Match, Rule, Table}
  Match::Any | All | AtLeast | Exact | NearPhrase
  Table { exclusions, rules } → first() / first_on_normalised()
```

| 领域 | 迁法 |
|---|---|
| 日历 | 排除 → Next 长短语 → Search 探针 → `AGENDA_TABLE`（双命中 / 裸日程） |
| 总结 | `SUMMARY_TABLE`：排除物 → 动词∩周 → 动词∩日 |
| 规划 | `Match` 串联 + kit≥2 门控（信号 2/3） |
| 取消 | `NearPhrase(slack=2)` + `Exact` |

---

## 2. 里程碑

| # | 内容 | 验收 |
|---|---|---|
| T1 | `intent_router.rs` + 原语单测 | 模块可编译 |
| T2 | 四领域迁入 | 日历/总结/规划/取消既有 CI 全绿 |
| T3 | 本文回填 | 文首已实施 |

---

## 3. 变更记录

| 版本 | 日期 | 说明 |
|---|---|---|
| v0.1 | 2026-11-24 | 初稿并落地 T1–T3 |

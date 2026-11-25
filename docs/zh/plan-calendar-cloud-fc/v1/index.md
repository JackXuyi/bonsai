# 方案：日历对话改云端 FC（退役规则腿）

> **状态**：已实施（2026-11-25）  
> **承接**：[plan-calendar.md](../../plan-calendar.md) §3.5  
> **一句话**：删除 `chat_turn_inner` 对 `tools::maybe_answer` 的前置劫持；新增 `CalendarToolsProvider`（`calendar_agenda` / `calendar_next` / `calendar_search`），仅 **云端 + `tools.calendar.enabled`** 挂载。

---

## 为何改

规则腿不写 `tool_call` 迹，与「工具调用须走云端 FC 且出现在调用日志」冲突；且与 AGENTS「能力一份权威（builtin ToolProvider）」平行。

## 行为

| 场景 | 行为 |
|---|---|
| 云端 + 日历开 | kit 挂日历三工具；模型 FC → `execute_audited` → 迹 + 审计 |
| 本地 / 日历关 | 不挂；不问 EventKit；不代答 |
| 会前提醒 job | **不动**（scheduler / `calendar::tick` 仍本地） |

成文模板复用 `tools::answer`；读库权威仍 `calendar::agenda_for_intent`。结果带 untrusted 标签。

## 相对 plan-calendar「日程永不出云」

本版显式修订：用户已开云端且打开日历时，**摘要可进当次云端 tool 结果**（与 mail 元数据同级）；不写 facts。

## 变更记录

| 版本 | 日期 | 说明 |
|---|---|---|
| v1 | 2026-11-25 | 退役对话规则腿 + CalendarToolsProvider |

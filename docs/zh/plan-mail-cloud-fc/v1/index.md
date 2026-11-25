# 方案：邮件仅云端 function calling（退役规则腿）

> **状态**：已实施（2026-11-25）  
> **承接**：[plan-mail/v1](../plan-mail/v1/index.md)  
> **一句话**：删除对话前置 `mail::maybe_answer`；仅 **云端模式且 `tools.mail.enabled`** 时挂 `MailToolsProvider`，经 `cloud_tool_loop` → `execute_audited`（生成链路必有 `tool_call`）。

---

## 变更

| 项 | 旧 | 新 |
|---|---|---|
| 本地规则代答未读 | 有 | **无** |
| 挂载条件 | `tools.mail.enabled` | **云端** + `tools.mail.enabled` |
| 调用日志 | 规则腿仅 `tool_intent` | FC 路径 `tool_call` + agent 审计 |

本地被问邮件：不代答、不挂工具；模型应诚实要求开云端。

与「标已读」增量见 [plan-mail/v2](../plan-mail/v2/index.md)（能力叠在同一 provider 上，挂载条件仍受本版约束）。

## 变更记录

| 版本 | 日期 | 说明 |
|---|---|---|
| v1 | 2026-11-25 | 退役规则腿；仅云端 FC |

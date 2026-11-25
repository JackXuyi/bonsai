# bonsai 中文文档索引

> AI / 人工默认从这里找入口。新方案 **MUST** 落在 `docs/<domain>/<version>/index.md`（见 `AGENTS.md` §7）；本页不重复正文。

## 工程约束

- [`AGENTS.md`](../../AGENTS.md) — 全项目 MUST/SHOULD
- [`runbook-l3-acceptance/v1`](runbook-l3-acceptance/v1/index.md) — 发版前真机短清单

## 架构路线

| 文档 | 说明 |
|---|---|
| [`roadmap-architecture-r2/v2`](roadmap-architecture-r2/v2/index.md) | **当前** R2 状态回填与下一优先 |
| [`roadmap-architecture-r2/v1`](roadmap-architecture-r2/v1/index.md) | 历史裁决 |
| [`roadmap-agent-capabilities.md`](roadmap-agent-capabilities.md) | 能力扩展主线（P2–P5） |
| [`research-report.md`](research-report.md) | 立项调研 |

## 已版本化方案（优先读最新 vN）

| 领域 | 最新 |
|---|---|
| Reliability / Intelligence / Autonomy | `plan-reliability-phase1/v1` · `plan-intelligence-phase2/v1` · `plan-autonomy-phase3/v1` |
| Chat-first / presence / agent inline | `plan-chat-first-ui/v3` · `plan-presence-im/v4` · `plan-chat-agent-inline/v2` |
| Files / mail / calendar FC | `plan-files/v3` · `plan-mail/v2` · `plan-calendar-cloud-fc/v1` · `plan-mail-cloud-fc/v1` |
| Jobs / skill auto | `plan-jobs/v1` · `plan-skill-auto-extract/v3` |
| 其它 | `plan-mcp-admin-split/v1` · `plan-app-workspace-memory/v1`（草案）等 |

## 扁平 `plan-*.md`（历史实施记录）

下列文件多为早期落盘；**已实施**者以文首状态为准，细节以代码与上表版本化为准。新增量请建 `docs/<domain>/vN/`，勿继续加长 flat 正文。

`plan-memory` · `plan-persona` · `plan-predict` · `plan-morph` · `plan-relationship` · `plan-onboarding` · `plan-calendar` · `plan-agent-mcp-skill` · `plan-dialogue-split` · `plan-db-split` · `plan-store-lock` · `plan-structured-routing` · `plan-skill-auto-extract`（M0–M2；M3 见 v3）· 及其余 `docs/zh/plan-*.md`

## 下一产品优先（非新垂直能力）

Chat-first 主路径稳态（对话 / 云端授权 / 任务恢复）→ L3 清单勾选 → 触及巨模块再拆。

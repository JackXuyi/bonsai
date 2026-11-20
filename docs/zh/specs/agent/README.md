# agent specs — MCP 调用 + Skill 支持（plan-agent-mcp-skill.md v0.4）

| spec | 里程碑 | 状态 |
|---|---|---|
| [spec-s1-mcp-core.md](spec-s1-mcp-core.md) | S1 rmcp 客户端核心 | ✅ 已实施（真机项待验） |
| [spec-s2-schema-admin.md](spec-s2-schema-admin.md) | S2 schema v8 + server 管理 | ✅ 已实施（真机项待验） |
| [spec-s3-pipeline-env.md](spec-s3-pipeline-env.md) | S3 三级渐进管线 + 环境段 | ✅ 已实施（真机项待验） |
| [spec-s4-approval-audit.md](spec-s4-approval-audit.md) | S4 审批钩子 + 审计 | ✅ 已实施（含 ask 模式端到端握手） |
| [spec-s5-skill.md](spec-s5-skill.md) | S5 Skill 核心 | ✅ 已实施（`skill_reload` 已裁） |
| [spec-s6-ui.md](spec-s6-ui.md) | S6 UI 交互 | ✅ 已实施（审批模式开关已在设置页） |
| [spec-s7-acceptance.md](spec-s7-acceptance.md) | S7 真机验收 + 文档收尾 | 🔨 CI 可验项全绿；真机项待验收 |

上游方案：`docs/zh/plan-agent-mcp-skill.md`（v0.4，D1–D12 全部拍板；实施记录与偏差见其 §8）。

验收口径：所有 `真机：` 前缀项必须在 `.app`（非 CI）环境跑一遍才算关闭本方案；其余项均已由 `cargo test`（278 测）/ `vitest`（70 测）/ `tsc` / `next build` 覆盖。

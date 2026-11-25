# bonsai（盆栽）

一个陪你一起长大的桌面 AI 伙伴。本地优先的记忆、有自己脾气的人格、彻底开源。你的数据永远不离开你的电脑。

**English** → [README.md](./README.md)

## 它是什么

介于「AI 陪伴」与「AI 助手」之间：先做真实有用的活，再谈关系连续性。反谄媚靠机制，不靠模型自觉。成长绑在理解准确度上，不绑互动时长。

不做恋爱伴侣、角色商店、无限对话时长。

## 架构（L0–L6）

```text
L6 呈现   脸 / 托盘 / Chat-first UI
L5 生成   人格注入 → 本地或云端对话；工具仅云端 FC
L4 决策   「要不要开口」与生成分离
L3 人格   persona 权威源，与模型无关
L2 记忆   本地 SQLite；夜间整理；Forget / 隐式检索
L1 感知   前台 App / idle 等，原文不出机（默认关）
L0 壳     Tauri 2 · macOS 先发
```

## 技术栈

- **壳**：Tauri 2（Rust）+ Next.js 14（`frontend/`，静态导出进 WebView）
- **记忆**：SQLite（WAL + 读写分离）
- **模型**：本机 Ollama sidecar（默认纯聊天）；云端 DeepSeek 等显式 opt-in 后才挂工具 / Skill

## 运行

前置：Rust 1.77+、Node.js 22+、[pnpm](https://pnpm.io) 10+。

```sh
pnpm llm:sidecar   # Ollama 运行时 → src-tauri/binaries/
pnpm llm:pull      # 默认模型到 .llm/models（可跳过，则规则兜底）
pnpm install
pnpm check         # cargo check
pnpm test          # Rust 单测
pnpm app           # tauri dev（前端 :1420）
pnpm app:build
```

PR / `main` 推送跑 [`.github/workflows/ci.yml`](.github/workflows/ci.yml)。工程约束见 [`AGENTS.md`](./AGENTS.md)。

## 当前能力（摘要）

已落地：托盘常驻、情绪脸与皮肤、三层记忆 + 夜间整理、人格与反谄媚、主动搭话配额、预测/形态/关系成长、onboarding 边界、日历/邮件（**仅云端 FC**）、文件/Web 工具、MCP + Skill、任务卡与恢复、Skill 自动提取草稿、诊断与写锁采样。

**下一优先**：Chat-first 主路径真机稳态（见 L3 清单），少开新垂直能力。未做：语音、完整屏幕 Computer Use、跨设备。

## 文档

- [中文文档索引](./docs/zh/index.md) — **推荐入口**
- [L3 真机验收短清单](./docs/zh/runbook-l3-acceptance/v1/index.md)
- [架构 R2 状态（v2）](./docs/zh/roadmap-architecture-r2/v2/index.md)
- [调研报告](./docs/zh/research-report.md)

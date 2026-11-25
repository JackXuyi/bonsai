# Bonsai 架构 R2 后续 Roadmap（状态回填）

| 文档信息 | 内容 |
|---|---|
| 版本 | v2 |
| 日期 | 2026-11-25 |
| 状态 | **生效** — 相对 [v1](../v1/index.md) 回填已实施项；v1 保留为历史裁决 |
| 一句话 | **Phase 1–3 代码主路径已落地；剩余主债是 L3 真机与文档索引，不是再堆垂直能力** |

---

## 0. 相对 v1 的基线纠正

| v1 §2.1 项 | v2 事实（2026-11-25） |
|---|---|
| `should_speak` UTC vs Local | **已修**（Reliability R4：`clock::now_local()` + `proactive_daily_usage`） |
| `forget_session` 漏删长期记忆 | **已修**（R1 Forget + Provenance；见 plan-reliability-phase1） |
| 任务仅标 `interrupted`、无恢复 | **已改善**（Autonomy A1：显式 `task_resume`；启动不自动续跑，D1） |

Phase 完成口径仍区分 L1/L2/L3；测试绿 ≠ 真机验收（见 [`runbook-l3-acceptance/v1`](../../runbook-l3-acceptance/v1/index.md)）。

---

## 1. 三阶段状态（摘要）

| 阶段 | 状态 | 残余 |
|---|---|---|
| Phase 1 Reliability | **已实施**（R1–R5 L2） | L3 删会话抽查 |
| Phase 2 Intelligence | **已实施**（I1–I3 L2；I4 可选） | I4 评测棘轮 / 隐式 L3 |
| Phase 3 Autonomy | **已实施**（A1–A5 L2；jobs 统一） | L3：杀进程恢复、写工具不双写 |

详细方案仍以各 `plan-*-phase*/v1` 为准；本页只做排期状态权威回填。

---

## 2. 下一迭代优先（产品打磨，少开新垂直能力）

1. **Chat-first 主路径稳态**：本地闲聊 → 云端 FC 授权 → 任务卡恢复（对照 L3 清单 A–E）。
2. **文档索引**：[`docs/zh/index.md`](../../index.md)；新方案一律 `docs/<domain>/vN/index.md`。
3. **触及才拆巨模块**（AGENTS §12）：`filetools` / `memory_search` / `context` / `calendar` / `consolidate`；禁止纯重构 sprint。
4. **Skill auto M3**：草稿淘汰 + 使用反馈（见 [`plan-skill-auto-extract/v3`](../../plan-skill-auto-extract/v3/index.md)）。

**明确缓做**：语音、完整 Computer Use、跨设备、Vector Hybrid（仍跟能力 roadmap 触发条件）。

---

## 3. 变更记录

| 版本 | 日期 | 说明 |
|---|---|---|
| v2 | 2026-11-25 | 回填 Phase 1–3 已实施；纠正 UTC/Forget/恢复基线；下一优先改为打磨 |
| v1 | 2026-11-24 | 初稿与分阶段方案锚点 |

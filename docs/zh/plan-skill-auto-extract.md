# 自动提取 Skill 能力方案（Auto-Extract Skill）

| 文档信息 | 内容 |
|---|---|
| 版本 | v0.3（已实施） |
| 日期 | 2026-11-25 |
| 状态 | **已实施**（M0–M3；云端增强仍不做） |
| M3 权威 | [`plan-skill-auto-extract/v3`](plan-skill-auto-extract/v3/index.md) |
| 上游输入 | `skill.rs` / `memory/scheduler.rs` / `dialogue/cloud_loop.rs` / AGENTS.md 模型与能力约束 |
| 定位约束 | 本地零工具调用；能力不重复实现；草稿默认禁用；SCHEMA 不 bump（只用 settings KV） |
| 一句话 | 成功任务入队 → 夜间/手动裸 chat 成包 → 用户确认后启用 |
| 用户裁决 | Q1–Q6 按推荐默认（2026-11-24 实施时闭合） |

---

## 0. 一句话结论

**事件轻量入队 + 夜间定时成包 + 用户手动即时触发**：从已留存的执行轨迹里筛出"成功且可复用"的任务，用裸模型（不带工具、不带 AgentKit）生成一份 SKILL.md 草稿，复用 `skill::create_skill_pack()` 落盘，默认**不启用**，用户在设置页确认后启用，后续用自然语言即可触发。

不做"每次对话都调模型生成"——成本、垃圾包、自噬循环三个问题都不可接受。

---

## 1. 背景与目标

（历史锚点保留；行号以实施时为准，权威实现见 `skill_auto.rs`。）

### 1.2 目标

1. 从对话与执行中自动沉淀可复用技能，用户零手写。
2. 生成物必须是**现有 skill 生态的一等公民**：能被 `skill_list` 列出、预览、启停、删除，启用后走既有注入链路。
3. 全程不新增平行能力：不重写校验、不重写脱敏、不重写落盘。

### 1.3 非目标

- 不生成"可执行脚本"——项目 D4 裁决：skill 是 prompt-only。
- 不让本地模型调用任何工具或挂载 skill。
- 不改变人格宪法 L1、不做自动启用。
- 不在本能力里重做记忆抽取。

---

## 2–4. 设计摘要

见初稿架构：T1 `enqueue_candidate` @ `task_finish(done)`；T2 `run_if_due` @ scheduler consolidate 尾；T3 `skill_extract_now`。状态键 `skill_auto_state` / `skill_auto_meta` / `skill_auto_enabled`。严禁投影 `trace_steps.prompt`。

---

## 5. 分期里程碑

| 期 | 交付 | 状态 |
|---|---|---|
| **M0 候选层** | KV、`enqueue` / `select` / `project` / `validate` + 单测 | ✅ |
| **M1 生成闭环** | `extract_candidate`、scheduler 挂接、上限与幂等；总开关默认关 | ✅ |
| **M2 管理面** | SkillSection 草稿/候选/开关、`skill_update` / extract / dismiss 命令 | ✅ |
| **M3 质量闭环** | 使用反馈 + 草稿淘汰（14 天）；云端增强不做 | ✅（见 v3） |

---

## 6. 落地记录（2026-11-24）

| 项 | 位置 |
|---|---|
| 核心 | `src-tauri/src/skill_auto/`（§12 拆分；M3 含 `gc_stale_drafts` / `record_skill_use`） |
| 入队 | `dialogue/cloud_loop.rs` `task_finish` 后 |
| 夜间 | `memory/scheduler.rs` consolidate 成功后 |
| 命令 | `mcp_admin::{skill_update,skill_extract_now,skill_auto_*}` |
| 落盘/改写 | `skill::{create_skill_pack,update_skill_pack}` |
| UI | `SkillSection.tsx` + `tests/SkillAuto.test.tsx` |

**门禁**：`cargo test --lib skill_auto::`；`pnpm check`；`tsc` + vitest SkillAuto / AgentSections。

**SCHEMA**：仍不 bump（当前库为 v17）；只用 settings KV。

**Q1–Q6**：草稿禁用；三触发；M1 本地 Background 腿裸 chat；≤2/夜；同链 ≥2 聚类；M2 管理面；M3 淘汰缓做。

---

## 7. 变更记录

| 版本 | 日期 | 说明 |
|---|---|---|
| v0.1 | 2026-11-23 | 初稿，待确认 |
| v0.3 | 2026-11-25 | M3 淘汰+使用反馈；模块目录化 |
| v0.2 | 2026-11-24 | M0–M2 落地；Q1–Q6 按推荐闭合 |

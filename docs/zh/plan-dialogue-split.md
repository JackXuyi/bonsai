# plan-dialogue-split — 拆分 `dialogue.rs` 巨石文件

| 文档信息 | 内容 |
|---|---|
| 版本 | v0.2（已实施） |
| 日期 | 2026-11-24 |
| 状态 | **已实施**（T1–T5）；D1–D4 按推荐闭合 |
| 上游输入 | 优化盘点（dialogue 巨石）、`AGENTS.md`（能力不重复实现）、`memory/` 目录范式、`plan-planning` / `plan-retry` / `plan-context` / `plan-summary-skill` |
| 定位约束 | 行为零变化；公开 API 零变化；能力不平行实现；本地零工具调用硬约束不动 |
| 一句话 | 把「一个文件里的对话管线」还原成「一个目录里的管线阶段」——只搬家，不改脑子 |
| 用户裁决 | **D1–D4** 均按推荐（2026-11-24）：`dialogue/` 目录；测试 `#[cfg(test)]` + 共享 re-export；`cloud_loop` 不二拆；禁止夹带逻辑改动 |

---

## 0. 一句话结论

`src-tauri/src/dialogue.rs` 现 **5250 行**：生产代码约 **2488 行**，单测约 **2762 行（占 53%）**。危机拦截、日历规则、上下文装配、总结接管、规划 / 云端工具循环、本地召回、流式包装、主动搭话、Tauri 命令与 70 个单测全部堆在同一文件。每加一个 plan（retry / planning / context / loops）都在同一战场抢行号，review 与冲突成本已经高于功能本身。

本方案只做一件事：**按管线阶段把文件拆成 `dialogue/` 目录模块**（对齐 `memory/` 先例），公开符号与管线顺序逐字节保持，测试随域迁走。不做逻辑重写、不做第二套 ReAct、不顺便「优化」prompt。

> 关键认知纠偏：巨石的痛点是**导航与冲突**，不是算法错了。`cloud_tool_loop` 已经是权威执行内核（plan-planning）；拆分时它整块搬家，禁止在拆分 PR 里改护栏数字、重试语义或意图路由。

### 关键取舍（先看这五条）

| 取舍 | 结论 | 理由 |
|---|---|---|
| 目录还是 `dialogue_*.rs` 平铺 | **`dialogue/` 目录**（`mod.rs` + 子文件） | 与 `memory/`、`llm/`、`persona/` 一致；平铺会在 `src/` 根再堆 8 个文件 |
| 拆分时要不要改行为 | **绝对不**。每个 T 的门禁是「既有 dialogue 单测全绿 + 无逻辑 diff」 | 否则拆分与修 bug 缠在一起，无法回滚 |
| `cloud_tool_loop` 要不要再拆 | **V1 不拆**。整文件迁到 `cloud_loop.rs`（含 plan/retry 助手） | 循环内部耦合高；再拆要改可见性与大量 `pub(crate)`，收益低 |
| 测试怎么放 | **按域跟代码走** + 共享 `tests/common` 夹具；禁止继续一个 2700 行 `mod tests` | 否则拆完生产代码，冲突面仍在测试半边 |
| 对外路径 | **`crate::dialogue::{...}` 保持**；`lib.rs` 的 `generate_handler!` 与 `spawn_proactive` 调用点零改或只改 `use` | 前端 / 文档引用的是命令名与事件，不依赖 Rust 文件路径 |

---

## 1. 现状锚点（全部实测，带行号）

### 1.1 体量

| 区段 | 行号（约） | 行数 | 内容 |
|---|---|---|---|
| 类型 / 常量 / Trace | 1–280 | ~280 | `DialogueOutcome` / `TaskCard` / `TraceRecorder` |
| 落库 / prompt 装配 | 281–463 | ~183 | `persist_model_turn` / `system_prompt` / `env_prefix` / `turn_budget` |
| 规划 + 工具助手 | 464–827 | ~364 | `plan_intent` / `submit_plan` / `should_retry` / `tool_specs`… |
| 云端循环 | 828–1320 | ~493 | `cloud_tool_loop` 主体 |
| 本地规则腿 | 1321–1560 | ~240 | `local_recall_answer` / `local_perception_answer` / `summarize_tool_result` |
| 入口包装 | 1561–1660 | ~100 | `chat_turn*` / `call_backend_with_trace` |
| 管线编排 | 1661–2013 | ~353 | `chat_turn_inner`（危机→日历→上下文→总结→云/本地→filter→落库） |
| 总结接管 | 2014–2162 | ~149 | `summary_turn` |
| 主动搭话 | 2163–2299 | ~137 | `proactive_*` / `spawn_proactive` |
| Tauri 命令 | 2300–2488 | ~189 | `dialogue_chat(_stream)` / history / trace |
| **单测** | 2489–5250 | **~2762** | `mod tests`，约 **70** 个 `#[test]` |

### 1.2 对外契约（拆分后必须保持）

| 符号 / 命令 | 消费方 | 约束 |
|---|---|---|
| `dialogue::chat_turn*` / `chat_turn_with_kit` | 内部 + 单测 | 签名不变 |
| `dialogue::spawn_proactive` / `PROACTIVE_SESSION` | `lib.rs`、`predict.rs` | 路径不变（`pub use` 再导出） |
| `dialogue_chat` / `dialogue_chat_stream` / `dialogue_history` / `turn_trace_*` / `dialogue_proactive_now` | `lib.rs` `generate_handler!`、前端 `commands.ts` | 命令名与 payload 不变 |
| 管线顺序 | 各 plan 文档 | 危机 → 日历规则 → context → 总结意图 → 云端循环/本地直答 → filter → 落库 |

### 1.3 根因

不是「缺少抽象层」，而是**历史增量全写进同一文件**：P0 对话装配 → 日历规则 → MCP/Skill → 规划 → retry → context，每一期都在 `dialogue.rs` 追加。测试也同文件膨胀，导致：

1. 任何对话相关 PR 必碰 5000 行文件 → merge 冲突；
2. review 无法按阶段阅读；
3. `pub(crate)` / 私有助手边界模糊，后续想抽公共逻辑更难。

---

## 2. 能力边界

### 2.1 做

| 能力 | 做到什么程度 |
|---|---|
| 目录化 | `dialogue.rs` → `dialogue/mod.rs` + 子模块（见 §4） |
| 公开 API 稳定 | 既有 `pub` / `pub async` / 命令全部从 `mod.rs` 再导出 |
| 测试随迁 | 按域拆到子模块或 `dialogue/tests/`；共享 `mem()` / `card()` 夹具 |
| 可见性整理 | 跨子模块助手统一 `pub(crate)`；不扩大成 `pub` |

### 2.2 明确不做

| 不做 | 原因 |
|---|---|
| 改 `cloud_tool_loop` 算法 / 护栏 / 重试次数 | 属 plan-retry / plan-planning；拆分 PR 禁止夹带 |
| 新建第二套执行循环或「更干净的 orchestrator 框架」 | 违反「能力不重复实现」 |
| 把日历规则从 `tools.rs` 再抄一份进 dialogue | 权威在 `tools::maybe_answer` |
| 把总结素材引擎迁进 dialogue | 权威在 `summary.rs`；dialogue 只保留 `summary_turn` 编排壳 |
| 前端改动 | 命令名不变，UI 无感 |
| schema / settings 迁移 | 纯代码搬家 |

---

## 3. 不变量（违反即回滚）

1. **行为等价**：同一输入 → 同一 `DialogueOutcome` 字段、同一落库行、同一 trace `step_kind` 序列。
2. **公开路径稳定**：`crate::dialogue::X` 与 Tauri 命令名不改。
3. **管线顺序不改**：`chat_turn_inner` 的阶段顺序与早退条件保持。
4. **本地零工具**：本地分支不出现 `tools[]` / 规划；拆分不得「顺便」给本地挂工具。
5. **单测集合不缩水**：原 `dialogue.rs` 内全部 `#[test]` 必须仍存在且通过（可改 `use` 路径）。
6. **拆分 PR 无行为 diff**：`git diff` 在逻辑上应几乎只有 `mv` + `mod` 声明 + 可见性；若出现算法行变更，必须拆出独立 commit 并说明。

---

## 4. 目标结构

```text
src-tauri/src/dialogue/
  mod.rs           # 模块根：pub use 再导出；文档总述；pub mod 子模块
  types.rs         # DialogueOutcome / TaskCard / TaskStep / ProactiveReply / 常量
  trace.rs         # TraceRecorder
  persist.rs       # lock_conn / persist_model_turn / filter_json / clip
  prompt.rs        # system_prompt / env_prefix / turn_budget / mood_str / turns_to_history
  cloud_loop.rs    # plan_* + should_retry + tool_* + cloud_tool_loop（整块）
  local_legs.rs    # local_recall_answer / local_perception_answer / summarize_tool_result / memory_search_args
  summary_turn.rs  # summary_turn / summary_round_prompt / today_observations
                   # （勿与 crate::summary 混淆：那边是素材引擎）
  turn.rs          # chat_turn* / chat_turn_inner / call_backend_with_trace
  proactive.rs     # proactive_text / iteration / spawn_proactive
  commands.rs      # 全部 #[tauri::command]
  tests_support.rs # #[cfg(test)] mem/card/with_conn 夹具（pub(crate)）
```

`lib.rs` 保持：

```rust
pub mod dialogue;
// generate_handler![ dialogue::dialogue_chat, ... ]
// dialogue::spawn_proactive(...)
```

### 4.1 依赖方向（禁止反向）

```text
types ← trace ← persist ← prompt
                ↘
turn → summary_turn / local_legs / cloud_loop / prompt / persist / trace
proactive → turn（或 prompt+persist，避免环）
commands → turn / proactive / persist
```

- `cloud_loop` **不得**依赖 `turn`（避免环）；需要的类型从 `types` / `trace` 注入。
- `chat_turn_inner` 是唯一编排者：调用各方，自己不实现工具执行细节。

### 4.2 预估拆完后单文件行数

| 文件 | 估产线（不含测） | 含测上限目标 |
|---|---|---|
| `types.rs` | ~80 | ~80 |
| `trace.rs` | ~200 | ~350 |
| `persist.rs` | ~120 | ~200 |
| `prompt.rs` | ~120 | ~250 |
| `cloud_loop.rs` | ~850 | ~1600 |
| `local_legs.rs` | ~250 | ~450 |
| `summary_turn.rs` | ~160 | ~350 |
| `turn.rs` | ~450 | ~900 |
| `proactive.rs` | ~140 | ~300 |
| `commands.rs` | ~200 | ~200 |
| `mod.rs` | ~80 | ~80 |

单文件生产代码目标：**&lt; ~900 行**；`cloud_loop` 允许到 ~850 产线（V1 不二拆）。

---

## 5. 分期（逐可编译单元提交）

| 期 | 内容 | 门禁 |
|---|---|---|
| **T0** | 本文评审；锁定 D1–D4 | 文档合入 |
| **T1** | `git mv dialogue.rs → dialogue/mod.rs`；抽出 `types` / `trace` / `persist`；其余仍在 `mod.rs` | `cargo test --lib dialogue::` 全绿；`cargo check` |
| **T2** | 抽出 `prompt` / `proactive` / `commands` | 同上；`lib.rs` handler 仍编译 |
| **T3** | 抽出 `cloud_loop`（含 plan/retry 助手整块） | 同上；规划 / 工具相关单测全绿 |
| **T4** | 抽出 `local_legs` / `summary_turn`；`turn.rs` 只留编排 + 入口 | 同上；总结 / 本地召回测全绿 |
| **T5** | 测试夹具 `tests_support`；按域迁测；删除空壳 | `cargo test --lib` 全量；对话相关测数量不减 |

> 顺序理由：先立目录与无依赖叶子（types/trace/persist），再迁命令与主动搭话（边界清晰），最后动最重的 `cloud_loop` 与编排核，降低每步 diff 面积。

每期提交信息建议：`refactor(dialogue): split …` —— 纯搬家，commit body 写明「行为无变更」。

---

## 6. 验收标准

| # | 场景 | 通过条件 |
|---|---|---|
| A1 | T1 完成后 | `dialogue.rs` 文件不存在；`dialogue/mod.rs` 存在；原公开符号均可 `crate::dialogue::` 引用 |
| A2 | 任意 Ti 完成后 | `cargo test --lib dialogue::` 与拆分前同集通过（允许路径变更，不允许删测） |
| A3 | 全量 | `cargo test --lib` 全绿；既有失败轮 / 规划 / 流式 / 危机测仍在 |
| A4 | API | `lib.rs` `generate_handler!` 列表命令名零变化；前端无需改 |
| A5 | 体积 | 每个子文件产线 &lt; 900（`cloud_loop` 例外书面认可）；不再存在 &gt; 3000 行单文件 |
| A6 | 行为抽检 | 手工或既有测覆盖：危机早退、日历 hit、总结接管、云端工具一轮、本地直答、主动搭话 spawn |

---

## 7. 风险矩阵

| 风险 | 概率 | 影响 | 缓解 |
|---|---|---|---|
| 搬家时改了可见性导致私有项测不到 | 中 | 中 | 跨模块助手统一 `pub(crate)`；测用 `super::` / `crate::dialogue::` |
| `cloud_loop` ↔ `turn` 循环依赖 | 中 | 高 | 依赖方向写死（§4.1）；需要的回调以参数注入，不反向 `use` |
| 大 PR review 困难 | 高 | 中 | 严格按 T1–T5 分期；每期只迁一类；禁止夹带行为改动 |
| `summary_turn.rs` 与 `crate::summary` 命名混淆 | 中 | 低 | 文件名固定 `summary_turn`；模块注释写明「编排壳，引擎在 summary.rs」 |
| git 历史 blame 断裂 | 低 | 低 | 使用 `git mv`；接受目录级历史，必要时 `git log --follow` |
| 测试夹具路径（`include_str!` persona.json） | 中 | 中 | `tests_support` 内用 `concat!(env!("CARGO_MANIFEST_DIR"), …)` 钉死路径 |

---

## 8. 决策点（全部闭合）

| # | 问题 | 裁决 | 落地 |
|---|---|---|---|
| **D1** | 目录 vs 平铺 | **`dialogue/` 目录**（2026-11-24） | `src-tauri/src/dialogue/` |
| **D2** | 测试放置 | **独立 `tests.rs` + `mod.rs` re-export**（2026-11-24） | 71 测全绿；后续可再按域拆 |
| **D3** | `cloud_loop` 再拆 | **不拆**（2026-11-24） | `cloud_loop.rs` ~886 行 |
| **D4** | 夹带清理 | **禁止逻辑改动**（2026-11-24） | 纯搬家 + 可见性/`use` |

---

## 9. 与上游 / 周边关系

| 文档 / 模块 | 关系 |
|---|---|
| `plan-planning` / `plan-retry` / `plan-context` | 逻辑权威仍在原条款；本方案只搬承载文件 |
| `crate::summary` | 总结**引擎**不动；dialogue 侧只保留 `summary_turn` 编排 |
| `crate::tools` | 日历规则路由权威不动；`chat_turn_inner` 继续调用 `tools::maybe_answer` |
| `crate::context` | 上下文装配权威不动；dialogue 继续 `context::build` |
| `AGENTS.md` | 遵守「能力不重复实现」：拆分 ≠ 新写平行管线 |
| `plan-loop-supervisor` | `spawn_proactive` 迁到 `proactive.rs` 后仍走既有 PanicGuard 接线 |

---

## 10. 实施记录

| 期 | 门禁 | 说明 |
|---|---|---|
| T1–T5 | `cargo test --lib dialogue::` **71 passed**；`cargo check --lib` 通过 | `dialogue.rs` → `dialogue/{mod,types,trace,persist,prompt,cloud_loop,local_legs,summary_turn,turn,proactive,commands,tests}.rs`；`lib.rs` handler 改 `dialogue::commands::*`（Tauri inventory 宏）；公开 `crate::dialogue::{chat_turn*,spawn_proactive,PROACTIVE_*}` 不变 |

## 11. 变更记录

| 版本 | 日期 | 说明 |
|---|---|---|
| v0.2 | 2026-11-24 | T1–T5 落地；D1–D4 按推荐闭合；产线最大 `cloud_loop` ~886 / `turn` ~471 |
| v0.1 | 2026-11-24 | 初稿：5250 行实况分区、`dialogue/` 目标树、依赖方向、T0–T5、A1–A6、D1–D4 |

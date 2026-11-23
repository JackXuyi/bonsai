# runbook-planning — 规划能力实施 runbook

| 文档信息 | 内容 |
|---|---|
| 版本 | v0.1 |
| 日期 | 2026-11-23 |
| 对应方案 | `docs/zh/plan-planning.md` v0.2（D1 默认确认 / D6 进度面板已裁决） |
| 范围 | T1–T7，逐可编译单元提交 |
| 基线 | `cargo test --lib` 452 passed / 0 failed（2026-11-23 实施前实测） |

---

## 0. 执行总原则（违反即停）

1. **一 T 一提交**：每个 T 独立编译、独立测试、独立提交；不把两个 T 混进一个 commit，不把 T 拆成不可编译的半成品提交。
2. **门禁不过不进下一期**：后端每 T 必过 `cargo check` + `cargo test --lib`；含前端的 T5/T6 加 `tsc --noEmit` + `vitest run`。
3. **提交规范**：subject 中文；**精确路径 `git add`**（禁止 `git add -A` / `git add .`）；**仅本地提交，不 push**。
4. **硬约束自查（每 T 提交前逐条过）**：
   - 本地模型零工具调用：`DialogueBackend::tool_client` 只有 `ClientKind::Cloud` 返回 `Some`，不新增本地工具路径。
   - 执行单点：所有动作走 `execute_audited`；禁止新建第二个执行循环、禁止规划器直接调 provider。
   - 过程不进消息流：`useChat` 的 `task_step` 只转 `planStore` 后 return，**绝不写 `chatStore.messages`**（11-22 裁决）。
   - schema 改动必同步 bump `SCHEMA_VERSION`（v10 教训），迁移幂等（`add_column_if_missing` + `IF NOT EXISTS`）。
   - 能力不重复实现：进度面板只消费 `task_step` / `plan_list`，不另写一份状态逻辑。
   - 文档与代码同步：某期若偏离方案，先改 `plan-planning.md` 再提交代码（用户惯例「代码改动后先更新设计文档」）。

---

## 1. 命令速查

```bash
# 后端（在 src-tauri/ 下）
cd /Users/xuyi/Desktop/git/bonsai/src-tauri
cargo check
cargo test --lib                 # 基线 452；每期只增不减

# 前端（含 T5/T6 时）
cd /Users/xuyi/Desktop/git/bonsai/frontend
npx tsc --noEmit
npm test                         # vitest run

# 仓库根：工作区核对与提交
cd /Users/xuyi/Desktop/git/bonsai
git status --short
git add <精确路径1> <精确路径2>
git commit -m "<中文 subject>"
```

> 注意：工作区有他人创建的未跟踪文件 `docs/zh/plan-skill-auto-extract.md`，**任何一期都不要 add 它**。

---

## 2. 分期实施卡片

### T1 — schema v14（数据底座）

| 项 | 内容 |
|---|---|
| 目标 | `tasks` 表承载计划，迁移兼容 v13 老库与 stamped-but-incomplete 库 |
| 改动文件 | `src-tauri/src/memory/db.rs` |
| 关键实现 | ① `SCHEMA_VERSION` 13→14 + 版本注释；② `SCHEMA` 里 `tasks` DDL 增 `plan_json` / `plan_mode` / `cancel_reason` / `confirm_mode` 四列；③ `migrate()` 补 4 行 `add_column_if_missing`（D1 默认值 `confirm_mode TEXT DEFAULT 'auto'`）；④ `PlanStep` / `PlanJson` 类型（serde，字段与方案 §4.2 一致）；⑤ `plan_put` / `plan_get` 读写函数（解析失败不 panic）；⑥ `TaskRow` + `task_list` 扩展新列；⑦ 单测：v13 老库升级、重复 migrate 幂等、plan_json 往返、task_list 新字段 |
| 门禁 | `cargo check` + `cargo test --lib`（≥452 全绿） |
| 验收映射 | A3/A11/A12/A14 的数据底座；本 T 以迁移测试为验收 |
| 提交 subject | `feat(planning): schema v14 计划字段与读写` |
| 回滚 | `git revert <T1-sha>`；注意回滚后 DB 仍带 v14 列（列冗余无害），但 `SCHEMA_VERSION` 回到 13 |

### T2 — 规划入口与计划轮

| 项 | 内容 |
|---|---|
| 目标 | 多步目标命中时产出结构化计划，失败降级；D1 确认门就位 |
| 依赖 | T1 |
| 改动文件 | `src-tauri/src/dialogue.rs`（+ 测试样本数据） |
| 关键实现 | ① `plan_intent(user_text, kit)` 纯规则，云端分支内调用；② 计划轮复用 `chat_with_tools`，tools 传专用 `submit_plan`（steps ≤6）；③ 协议特判：解析成功 → `tasks.plan_json` + `plan_mode='planned'` → 构造 `assistant(tool_calls)+tool_result` 接回现有循环；④ `plan_confirm_gate()` 恒 `Allow`（`auto` 直放、`manual` 只留痕不阻塞）；⑤ 失败/超步 → `plan_mode='none'` 降级无感；⑥ 20 正 / 15 负样本 CI |
| 门禁 | `cargo check` + `cargo test --lib`（全绿） |
| 验收映射 | A1 / A2 / A8 / A9 / A11 / A12 |
| 提交 subject | `feat(planning): 规划入口与计划轮` |
| 回滚 | `git revert <T2-sha>`；T1 数据列保留 |

### T3 — 状态桥

| 项 | 内容 |
|---|---|
| 目标 | 执行结果按步骤回写 `plan_json`，审计与计划对齐 |
| 依赖 | T2 |
| 改动文件 | `src-tauri/src/dialogue.rs` |
| 关键实现 | ① 每次 `execute_audited` 成功/失败后更新对应 step 的 `status` / `attempts` / `tool_call_id`（关联真实审计行）；② 计划外工具追加 `tool_hint="*"` 的 done 步骤，保证计划行数与审计一致；③ `task_step` 事件形状不变（D6 后续消费）；④ 不双写状态：`tool_calls` 是权威，`plan_json.status` 只补语义 |
| 门禁 | `cargo check` + `cargo test --lib` |
| 验收映射 | A3 / A10 |
| 提交 subject | `feat(planning): 计划步骤状态桥` |
| 回滚 | `git revert <T3-sha>` |

### T4 — 失败恢复与取消

| 项 | 内容 |
|---|---|
| 目标 | 有界重试（仅 read ≤1 次）、预算跳过、用户取消 |
| 依赖 | T3 |
| 改动文件 | `src-tauri/src/dialogue.rs`、`src-tauri/src/mcp_admin.rs`（`AgentKit.canceled`）、`src-tauri/src/lib.rs`（`dialogue_cancel` 命令） |
| 关键实现 | ① read 档失败重试 1 次 + 500ms 退避，ask/forbidden/写操作永不重试；② `bad_args` 不重试、`denied` 标 skipped；③ 8/6 护栏触发 → 剩余 skipped + `cancel_reason='budget'`；④ `AgentKit.canceled: AtomicBool`，循环每轮/每步前检查；⑤ 自然语言「别做了/停/取消」→ `cancel_reason='user'`；⑥ `task_finish(status='canceled')`，已执行步骤不回滚 |
| 门禁 | `cargo check` + `cargo test --lib` |
| 验收映射 | A4 / A5 / A6 / A7 |
| 提交 subject | `feat(planning): 失败恢复与取消` |
| 回滚 | `git revert <T4-sha>`；注意 Tauri 命令暴露同步移除 |

### T5 — trace 可读化

| 项 | 内容 |
|---|---|
| 目标 | `/trace` 时间线展示计划卡与步骤状态 |
| 依赖 | T4 |
| 改动文件 | `src-tauri/src/memory/db.rs` 或 trace 模块（`step_kind` 三枚举）、`frontend/src/app/trace/...`（时间线计划卡） |
| 关键实现 | ① `step_kind` 增 `plan_submit` / `step_state` / `retry`；② 前端时间线按新枚举渲染计划卡（目标 + 步骤状态 + 关联 `tool_call_id`） |
| 门禁 | `cargo test --lib` + `npx tsc --noEmit` + `npm test` |
| 验收映射 | A8 / A10 |
| 提交 subject | `feat(planning): trace 计划卡可读化` |
| 回滚 | `git revert <T5-sha>` |

### T6 — 进度面板（D6）

| 项 | 内容 |
|---|---|
| 目标 | 工具页只读进度卡 + 首屏回放，**过程仍不进对话气泡** |
| 依赖 | T3（可并行于 T4/T5） |
| 改动文件 | `src-tauri/src/lib.rs`（`plan_list` 命令）、`frontend/src/lib/commands.ts`、`frontend/src/lib/types.ts`、`frontend/src/lib/stores/planStore.ts`（新）、`frontend/src/hooks/useChat.ts`、`frontend/src/components/tools/PlanProgressCard.tsx`（新）、`frontend/src/components/tools/ToolsPane.tsx`、`frontend/tests/...` |
| 关键实现 | ① `plan_list(limit)` 复用 `db::task_list` + 解析 `plan_json`；② `planStore` 按 `seq` 更新步骤、去重；③ `useChat` 的 `task_step` 转 store 后 return，**不写消息流**；④ `PlanProgressCard` 插入 `ToolsPane`（`ToolCallRecords` 前），只读不可勾选；⑤ 样式用 CSS 变量语义 token；⑥ 单测覆盖事件乱序/重复 id/终态对齐 + 卡片渲染 |
| 门禁 | `cargo test --lib` + `npx tsc --noEmit` + `npm test` |
| 验收映射 | A13 / A14 / A15 |
| 提交 subject | `feat(planning): 工具页计划进度面板` |
| 回滚 | `git revert <T6-sha>`；注意 `useChat` 回退后 `task_step` 恢复忽略，面板消失但后端不受影响 |

### T7 — 真机验收与文档回填

| 项 | 内容 |
|---|---|
| 目标 | DeepSeek 真连验证全链路，回填方案实施记录 |
| 依赖 | T4 / T5 / T6 |
| 改动文件 | `docs/zh/plan-planning.md`（§13 实施记录）、本 runbook（实施记录表） |
| 验收清单 | ① 20 正样本命中率 ≥90%、15 负样本 0 误触发；② 3 步计划全部成功 → 每步 `tool_call_id` 指向真实审计行；③ read 步骤失败重试 1 次成功、写失败不重试；④ 「别做了」→ canceled + 剩余 skipped；⑤ 护栏触发 → budget 收尾；⑥ `plan_confirm=manual` 本版不阻塞（A12）；⑦ 面板实时推进 + 首屏回放一致；⑧ 对话气泡全程只有最终回复 |
| 门禁 | 真机清单全过 |
| 提交 subject | `docs(planning): 真机验收与方案回填` |
| 回滚 | 文档提交无需回滚代码；若验收失败，回退对应 T 后重做 |

---

## 3. 实施记录（每 T 提交后回填）

| T | commit / subject | 门禁结果 | 真机项 | 备注 |
|---|---|---|---|---|
| T1 | `feat(planning): schema v14 计划字段与读写` | `cargo check` 通过（1 个既有 dead_code warning）；`cargo test --lib` 454 passed | — | 基线 452 → 454（新增 2 个迁移/往返单测） |
| T2 | `feat(planning): 规划入口与计划轮（D1 确认门）` | `cargo check` 通过；`cargo test --lib` 462 passed（20 正/15 负样本 + 6 个计划轮单测） | — | 基线 454→462；计划轮云端专属，`submit_plan` 不进 catalog |
| T3 | `cdf1ea8` `feat(planning): 计划步骤状态桥` | `cargo check` 通过；`cargo test --lib` 464 passed | — | 基线 462→464；按执行序号对齐，审计 id 前后水位关联，计划外追加 |
| T4 | `010dfba` `feat(planning): 失败恢复与取消` | `cargo check` 通过；`cargo test --lib` 469 passed | — | 基线 464→469；AgentKit.canceled + 全局注册表 + dialogue_cancel 命令 |
| T5 | `d84035f` `feat(planning): trace 计划卡可读化` | `cargo test --lib` 469 passed；`npx tsc --noEmit` 0 错；`vitest run` 148 passed | — | 后端补 `step_state` 记录点（`bridge_plan_step` 返回审计 id）；前端类型补三枚举 + 计划卡（`PlanSubmitBlock`/`PlanStateBlock`）+ 7 个解析/渲染单测 |
| T6 | `7b27550` `feat(planning): 工具页计划进度面板` | `cargo test --lib` 469 passed；`npx tsc --noEmit` 0 错；`vitest run` 165 passed | — | 148→165：`planStore`（事件乱序/重复/终态对齐 10）+ 卡片（首屏回放/实时推进/只读/历史/空态 5）+ 消息流隔离 A15（2）。`plan_list` 实装在 `mcp_admin.rs`（命令都住模块里，`lib.rs` 只登记）；`task_canceled` 无 id → store 归到 current；新增 `--ok/--warn/--danger` 语义色 |
| T7 | | | | |

---

## 4. 常见坑（实施前必读）

- **schema 只 bump 不迁移 = v10 事故重演**：`SCHEMA` 是 `CREATE TABLE IF NOT EXISTS`，老库的 `tasks` 表不会自动增生列；`migrate()` 里的 4 行 `add_column_if_missing` 缺一不可。
- **迁移要在 `SCHEMA` 之后**：`tasks` 表由 `SCHEMA` 建出，老库缺表时先 ALTER 会报 `no such table`；补列语句必须放在 `conn.execute_batch(SCHEMA)?` 之后。
- **`tool_call_id` 必须是真实审计行**：进度可追溯的底线，禁止用自增序号伪造；计划外步骤也要有真实 `tool_calls` 行。
- **`submit_plan` 是协议工具不是能力**：不进 `agent_tools.rs` 的 provider catalog，只由循环特判，否则违反「能力不重复实现」。
- **`manual` 本版不得真的阻塞**：`plan_confirm_gate()` 恒 `Allow`；A12 单测锁死「设了 manual 也不卡」。
- **面板首屏与实时态的来源分工**：首屏 `plan_list` 建基线，执行中 `task_step` 增量覆盖，终态以 `tasks` 表为准；不做定时轮询。
- **`task_canceled` 事件不带 id**（后端只发 `reason`）：`planStore` 把它归到 `current`；新增事件若要带 id 再改 store 的 `asEvent`，不要在组件里猜。
- **`plan_json` 嵌套字段仍是 snake_case**：`TaskRow` 的 `rename_all` 不下钻，前端必须经 `progressFromRow` 显式映射，不能直接当 camelCase 用。
- **tsc 是前端唯一可靠门禁**：WorkBuddy 宿主 shell 里 `next build` 必败（fs shim），只跑 `tsc --noEmit` + `vitest run`。

---

> 本 runbook 随实施推进更新；每 T 提交后回填 §3 记录表。若方案变更先改 `plan-planning.md` 再改本文件，最后动代码。

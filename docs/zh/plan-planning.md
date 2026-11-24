# plan-planning — 规划能力：显式计划、任务分解与进度跟踪

| 文档信息 | 内容 |
|---|---|
| 版本 | v0.4（真机验收） |
| 日期 | 2026-11-24 |
| 状态 | **已实施**（T1–T7）；「现状」锚点保留作历史 |
| 上游输入 | `docs/zh/roadmap-agent-capabilities.md`（G-09 任务模式 / G-08 常驻任务 / C5 规划与长任务）、`docs/zh/plan-task-card.md`（v0.2 工具分流裁决）、能力盘点（2026-11-23） |
| 定位约束 | 桌面养成系 AI 搭档；本地 LLM 优先、零工具调用；助手非伴侣；危险动作 Ask-first；能力不重复实现；一切以 AI 优先 |
| 用户裁决 | **D1**：计划默认确认（auto 直接执行，先干活）；**D6**：做进度面板（计划/进度进独立管理面，不进对话气泡）。两项均为 2026-11-23 用户拍板 |

---

## 0. 一句话结论

现有 `cloud_tool_loop` **已经是一个事实上的 ReAct 内核**（模型选工具 → Rust 执行 → 结果回传 → 再决策）。所以本方案要补的不是「再写一个 ReAct 引擎」，而是给它加一层**显式规划外壳**：让云端模型在开始前产出一份**结构化计划**（目标 → 步骤），执行中维护**步骤状态**，支持**失败恢复**与**用户中断**；执行引擎、审批、审计全部复用现有单点，一行不重写。

> 关键认知纠偏：通用 agent 的「ReAct / 目标分解 / plan / todo」是四件独立能力；在 bonsai 的约束下，它们收敛为**一个内核 + 一层状态**：内核是现成循环，状态层是新增的计划实体。若照抄四套机制，会同时违反「能力不重复实现」和「本地零工具调用」两条硬约束。

### v0.2 用户裁决落地摘要

| 裁决 | 结论 | 对方案的改动 |
|---|---|---|
| **D1 确认模式** | 计划**默认确认 = auto 直接执行**（先干活不打断）；**后续可扩展为手动确认** | 新增 `plan_confirm` 配置位（`auto` 默认 / `manual` 扩展位），默认路径零阻塞；manual 模式做成一个**独立后续可插拔的确认门**，本版只留接口不实现 UI 确认卡 |
| **D6 进度面板** | 做进度面板，但**不进对话气泡**——11-22「调用过程不进消息流」裁决维持 | 进度从「只在 `/trace`」扩展为「`/trace` 明细 + 工具页一个**计划进度卡**实时展示」；新增 `plan_list` 命令 + `planStore` + `PlanProgressCard`，气泡仍只显示结果 |

> 两者不冲突：D1 管「执行前要不要等用户」，D6 管「执行中用户去哪看进度」。默认不等（auto）+ 有地方看（面板）= 先干活且可观测。

---

## 1. 现状锚点（全部实测，带行号）

| 现状 | 位置 | 对本方案的约束 |
|---|---|---|
| `cloud_tool_loop` 已是隐式 ReAct：模型 `tool_calls` → 逐个 `execute_audited` → `tool_result` 回传 → 循环 | `dialogue.rs:373-593` | **不得新建第二个执行循环**；规划只产出「计划 + 状态」，动作仍走这里 |
| 循环护栏：工具调用 ≤8（`CLOUD_TOOL_CALL_CAP`）、HTTP 请求 ≤6（`CLOUD_REQUEST_CAP`） | `dialogue.rs:310-313` | 计划步数必须受同一预算约束；不能在计划里排 20 步然后被护栏截断 |
| 任务卡 `tasks` 表已存在但**只是审计外壳**：goal 取原话前 80 字、status（running/done/failed）、tool_calls 计数；`tool_calls.task_id` 关联 | `memory/db.rs:276-285`、`dialogue.rs:416-419,500-519,576-591` | 计划要复用/扩展这张表，**不另起炉灶**；步骤明细仍落 `tool_calls` |
| 任务卡在 11-22 被用户裁决**退出消息流**：前端 `useChat.ts:44-48` 明确忽略 `task_step` 事件 | `useChat.ts:5,44-48`、`plan-task-card.md:206` | **计划/进度不得进对话气泡**；只能在 `/trace` 页或独立面板展示。此裁决不可推翻 |
| Channel 事件协议仍在：`task_start` / `task_step` / `task_done`（前端忽略，协议兼容保留） | `dialogue.rs:509-514,552-562,588-589` | 计划进度事件沿用同一 Channel 形状，前端可选接入，不破坏现有忽略逻辑 |
| 管线顺序：危机拦截 → 日历规则 → 上下文 → 总结意图 → 云端循环/本地直答 → filter → 落库 | `dialogue.rs:911-1212` | 计划触发点只能在「总结意图」之后、云端循环之内；本地路径永不规划 |
| 本地模型零工具调用：`DialogueBackend::tool_client` 仅 `ClientKind::Cloud` 返回 Some | `persona/eval.rs:158-166`、AGENTS.md 硬约束 | **规划能力云端专属**；本地模式遇到多步目标 → 诚实告知需开启云端，不伪装执行 |
| `execute_audited` 是唯一执行入口：validate_args → 权限三档门 → 轮内缓存 → 执行 → 审计 | `mcp_admin.rs:515-669` | 计划里的每一步**必须**经它；禁止计划引擎直接调 provider |
| 权限三档：`read` 直行 / `ask` 强制 AskFirstGate（30s 超时拒）/ `forbidden` 永拒 | `agent_tools.rs:65-82`、`mcp_admin.rs:550-570` | 计划执行遇到 ask 档仍逐个弹卡，**不支持一揽子授权**；计划不能绕过审批 |
| 单步失败当前**不重试**：错误脱敏后作 `tool` 消息回传，让模型自行决策 | `dialogue.rs:534-550`、`plan-task-card.md:102` | 失败恢复是本方案要补的增量，但只补「有界重试 + 换策略提示」，不改「回传真实失败」的诚实原则 |
| 人格全程注入云端循环 system prompt（D6 裁决） | `dialogue.rs:383-389`、`plan-task-card.md:129-135` | 计划段的注入不得稀释人格；计划指令走 system 尾段，优先级声明低于 L1 宪法 |
| 画像固定注入 prompt（≤200 字），每轮都有 | `persona/prompt.rs:97-121` | 计划可利用画像做目标消歧（如「我的项目」），但不得把画像原文塞进计划 JSON 外发审计 |
| schema 当前 v13（画像表）；改 schema 必须同步 bump `user_version`，v10 有惨痛教训 | `memory/db.rs:20-36,391-448` | 本方案用 **v14**；迁移走 `add_column_if_missing` + `IF NOT EXISTS` 幂等范式 |
| 能力线只存在于调研文档，代码里 growth 只有形态线（`morph.rs`）与关系线（`relationship.rs`），**无能力线实体** | `research-report.md:262-268`、`src-tauri/src/` 无 `ability.rs` | 计划完成质量可**预留**能力线证据出口，但不得在方案里假装有能力线可写入 |
| `tasks` 表数据**未暴露给前端**：`lib.rs` 的 `generate_handler!` 无 task 命令、`commands.ts` 无 task 封装；`db::task_list` 存在但无人调用 | `lib.rs:241`、`commands.ts`、`db.rs:1561` | 进度面板必须新增 `plan_list` 命令 + 前端类型化封装；这部分是 D6 的**净新增工作量**，不计入「零改动」 |
| 前端 store 用 zustand，落在 `lib/stores/`（`chatStore` / `uiStore` / `llmStore` / `faceSkinStore`）；工具页卡片群在 `ToolsPane.tsx` 顺序排列 | `frontend/src/lib/stores/`、`components/tools/ToolsPane.tsx:18-48` | 进度面板按既有惯例加 `planStore.ts` + `PlanProgressCard.tsx` 插入 `ToolsPane`，不新开页面 |

---

## 2. 能力边界与取舍（做什么 / 不做什么）

### 2.1 通用 agent 四种规划形态在 bonsai 的去留

| 通用能力 | 通用 agent 做法 | bonsai 裁决 | 理由 |
|---|---|---|---|
| **ReAct 循环** | 模型自主 thought→action→observation | **已有，不重造** | `cloud_tool_loop` 就是；只补显式状态 |
| **目标分解** | 模型把目标拆成子任务树 | **改造**：只产出**扁平步骤列表**（≤6 步），不做树、不做依赖图 | 2B/云端小模型产树不可靠；扁平列表可被现有 `tool_calls` 审计直译 |
| **plan 模式** | 先出完整计划、用户确认、再执行 | **D1 裁决**：默认**确认即执行**（auto，不阻塞）；`manual` 手动确认做可扩展位，本版只留接口不实现 | 「先干活」定位；用户 2026-11-23 拍板「默认确认，后续可扩展手动」 |
| **todo 进度** | 侧栏任务列表可勾选 | **D6 裁决**：做**进度面板**（工具页计划进度卡 + `/trace` 明细），**不进对话气泡、不做可勾选动作** | 用户 2026-11-23 拍板要面板；11-22「过程不进消息流」裁决维持——面板是管理面，不是把过程塞回聊天 |

### 2.2 明确不做（本方案范围外）

- **自主长期目标 / 后台连续执行**：那是 G-08 常驻任务（P3-5），本方案只解决「一轮对话内的多步目标」。
- **多 Agent 分工**（G-16）、**Computer Use**（G-14）、**浏览器自动化**（G-15）：定位取舍，不变。
- **让本地 2B 模型做规划**：零工具调用硬约束 + 2B 不可靠；本地路径遇到复杂目标走诚实降级。
- **计划持久化跨会话**：计划生命周期 = 一次对话轮（task 行随轮结束写终态），不做跨天 todo。
- **手动确认 UI**（D1 扩展位）：本版 `plan_confirm=manual` 只留配置与门接口，不做确认卡；待用户后续要求再启用（避免现在就打断「先干活」）。
- **可勾选 / 可编辑的 todo 操作**：进度面板只读展示，不支持勾选完成、改步骤、拖动排序——进度由真实执行产生，不是用户手填的清单。

---

## 3. 总体架构

```
对话管线（dialogue.rs chat_turn_inner，改动仅限云端分支内部）
  危机 → 日历规则 → 上下文 → 总结意图（均不动）
    │
    └─ 云端模式且有 kit 能力：
       ┌─ 【新增 L1 规划入口】plan_intent(user_text, kit)  ← 轻量规则判定是否需要计划
       │     命中 → 一次「计划轮」：模型产出结构化 plan（goal + steps[]）
       │     未命中 → 直接进现有循环（单步任务零额外开销）
       │     【D1 确认门】plan_confirm=auto（默认）直接放行；manual 扩展位待启用
       │
       ├─ 【现有内核，不重写】cloud_tool_loop：
       │     每步 = execute_audited（权限/缓存/审计全复用）
       │     +【新增 L2 状态桥】把执行结果按顺序写回 plan 的步骤状态
       │     +【新增 L3 失败恢复】有界重试（read 档）/ 换策略提示 / 步标 failed
       │     +【新增 L4 中断】每轮循环前检查 cancel 标志
       │
       ├─ 收尾：计划终态写 tasks 表 + 进度事件走既有 Channel
       │        最终回复仍由模型以人格口吻产出（D6 不变）
       │
       └─ 【D6 观测面，独立于消息流】
              task_step 事件 → planStore（zustand）→ 工具页 PlanProgressCard 实时展示
              tasks 表 → plan_list 命令 → 面板首屏/刷新回放
              /trace 页继续承载逐步明细（唯一细粒度视图，不变）
```

**分层原则**：

1. **L1 规划入口**是纯决策，不执行任何动作 —— 符合「一切以 AI 优先」（默认交付面是模型产出）与「能力不重复实现」（不复制工具逻辑）。
2. **L2/L3/L4** 是对现有循环的**增强**，不是替代；所有动作仍单点走 `execute_audited`。
3. 计划是**审计与预期的外壳**，不是第二套执行引擎 —— 与 v0.2 把任务卡降级为外壳的裁决一脉相承。
4. **D6 观测面与执行面解耦**：进度面板是只读管理面，通过既有 `task_step` 事件与新增 `plan_list` 命令取数，不侵入对话管线、不改消息流。

---

## 4. 数据模型（schema v14）

### 4.1 扩展 `tasks` 表（不新建 plan 表）

```sql
-- v14 新增列（add_column_if_missing，向后兼容）
-- plan_json: 结构化计划 {"goal": "...", "steps": [{"seq":0,"intent":"...","tool_hint":"fs_read","status":"pending|running|done|failed|skipped","attempts":0}]}
ALTER TABLE tasks ADD COLUMN plan_json     TEXT;              -- NULL = 单步任务（无计划），无额外开销
ALTER TABLE tasks ADD COLUMN plan_mode     TEXT NOT NULL DEFAULT 'none'; -- 'none' | 'planned'
ALTER TABLE tasks ADD COLUMN cancel_reason TEXT;                         -- 'user' | 'budget' | NULL
ALTER TABLE tasks ADD COLUMN confirm_mode  TEXT NOT NULL DEFAULT 'auto'; -- D1: 'auto'（默认）| 'manual'（扩展位，本版不触发）
```

**为什么扩展 tasks 而不新建 `plans` / `plan_steps` 表**：

- 「能力不得重复实现」——任务与计划是同一件事的两种深度，`tasks` 已是循环的外壳，加列即可承载计划。
- 步骤明细已有 `tool_calls.task_id`，计划步骤状态可从 `tool_calls` 派生 + `plan_json` 的 `status` 补语义，**不存两份状态**。
- v10 教训：schema 改动越少越安全；加 4 列 + bump 版本号是最小变更。

**关于 `confirm_mode`（D1）**：

- 默认 `auto`——计划产出后直接执行，不等用户点击（落地「先干活」，与现有 `AutoAllowGate` 默认放行的哲学一致）。
- `manual` 是**扩展位**：本版写入 `tasks.confirm_mode` 留痕、门接口就位，但**没有确认卡 UI、不阻塞执行**；后续要启用时，只需在计划轮后加一个 `AskFirstGate` 式门 + 复用现有 `ApprovalBroker` 事件通道，无需再动 schema。
- 「默认确认」指**系统默认已确认可执行**，不是「默认弹窗等确认」——D1 原话语义按前者落地（先干活不打招呼）。

### 4.2 计划 JSON 结构（写入 `plan_json`，供 trace 页展示）

```json
{
  "version": 1,
  "goal": "把这两个文件合并成一份摘要",
  "steps": [
    { "seq": 0, "intent": "读第一个文件", "tool_hint": "fs_read", "status": "done", "attempts": 1, "tool_call_id": 12 },
    { "seq": 1, "intent": "读第二个文件", "tool_hint": "fs_read", "status": "done", "attempts": 1, "tool_call_id": 13 },
    { "seq": 2, "intent": "写入摘要文件", "tool_hint": "fs_write", "status": "done", "attempts": 1, "tool_call_id": 14 }
  ],
  "replans": 0
}
```

- `tool_hint` 是**预期**不是约束：模型实际调了别的工具也允许（记 trace，不阻断）；但计划外的 `ask`/`forbidden` 工具仍走审批。
- `tool_call_id` 关联 `tool_calls.id`（真实审计行），进度可追溯、不造数据。
- `status` 五态：`pending / running / done / failed / skipped`；终态由 `task_finish` 写 `done/failed/canceled`。

### 4.3 迁移

在 `memory/db.rs` 的既有迁移段（`add_column_if_missing` 序列，`db.rs:409-439`）追加 4 行，并把 `SCHEMA_VERSION` 从 13 改为 14（`db.rs:36`）。**不加新表**，因此 `stamped-but-incomplete DB` 的自愈逻辑（v10/v13 教训）不受影响。

---

## 5. 模块设计

### 5.1 L1 规划入口：`plan_intent()`（新函数，放 `dialogue.rs`）

纯规则判定，**零模型、零工具、零成本**，命中才进计划轮：

| 信号 | 判定 | 例 |
|---|---|---|
| 显式指令 | 「帮我…然后…」「先…再…」「规划/列步骤/分几步/一起做」 | 「先查下周三日程，再整理下载目录」 |
| 多动作 | 句中 ≥2 个动作动词（查/读/找/整理/写/总结/发…）且kit 有 ≥2 类 provider | 「查一下邮件并记到备忘录」 |
| 多工具预期 | 目标含两个以上 kit 工具域关键词（文件/网页/日程/记忆） | 「把网页内容存成文件」 |

不命中（**不产生计划轮，零额外 token**）：单轮问答、纯回忆（`memory_search`）、日历单查、总结意图（已被前置路由接管）、危机拦截、本地路径。

> 误触发控制：沿日历规则路由的 CI 惯例，建 20 正 / 15 负样本集，锁定命中率 ≥90%、误触发率 ≤10%。

### 5.2 计划轮：结构化 plan 产出

在云端循环的**第一次请求之前**插入一次轻量请求（复用 `chat_with_tools`，但 tools 传一个专用 `submit_plan` 工具）：

- `submit_plan` 入参 schema：`{ goal: string, steps: [{ intent: string, tool_hint: string }] }`，steps ≤6。
- 模型在计划轮里只提交 plan，不执行任何动作；`execute_audited` **不认识** `submit_plan`（它是协议工具，不是能力），由循环特判：解析成功 → 写 `tasks.plan_json` + `plan_mode='planned'` → 构造 `assistant(tool_calls) + tool_result("计划已记录，开始执行")` 接回现有循环。
- 计划轮失败/解析失败/超步数 → **降级为现有无计划循环**（`plan_mode='none'`），不阻断对话、不报错给用户。

这满足「能力不重复实现」：计划轮不新增执行分支，只是给循环喂一条特殊的 tool 消息。

**D1 确认门**插在「计划提交成功」与「接回执行循环」之间：

- 从 settings 读 `plan_confirm`（默认 `auto`），写入本轮 `tasks.confirm_mode` 留痕；
- `auto`：**直接放行**，零等待、零 UI（本版唯一实际路径，落实「默认确认」）；
- `manual`：本版只把门调用点预埋为一个可插拔函数 `plan_confirm_gate()`（返回 `Allow`），**不弹卡、不阻塞**；后续启用时在此接 `AskFirstGate` + 复用 `ApprovalBroker` 的 `tool:approval_required` 事件通道即可，不触碰执行循环。
- 无论哪档，`ask`/`forbidden` 档**工具自身的审批**照旧逐个走 `execute_audited`，与计划确认门是两件事，互不替代。

### 5.3 L2 状态桥：步骤状态回写（现成结构上加几行）

在 `cloud_tool_loop` 的每次 `execute_audited` 成功/失败分支（`dialogue.rs:522-550`）之后：

- 取当前 `plan_json`，按执行序号把对应 step 标 `running → done/failed`，写回 `attempts` 与 `tool_call_id`；
- 计划外工具（不在 `tool_hint` 中）：仍执行、仍记审计，但在 plan 里追加一条 `status=done` 的「计划外步骤」（`tool_hint="*"`），保证计划与审计行数一致；
- 每次状态变化仍发 `task_step` Channel 事件（**D6 后前端不再忽略**：由 `planStore` 消费驱动进度面板；事件形状不变，旧前端忽略逻辑保持兼容）。

### 5.4 L3 失败恢复：有界重试 + 换策略

| 场景 | 策略 | 约束 |
|---|---|---|
| `read` 档工具失败（超时/暂时不可达） | 最多重试 **1 次**（总 attempts ≤2），指数退避 500ms | 不重试 `ask`/`forbidden` 档；不重试写操作（避免重复副作用） |
| 重试仍失败 | 步骤标 `failed`，错误作 `tool` 消息回传，模型换工具或收尾 | 沿现有「回传真实失败」诚实原则 |
| 参数校验失败（`bad_args`） | 不重试（模型臆造参数），直接回传校验错误让模型修正 | 遵循 v0.2「校验在门之前」 |
| 审批拒绝（`denied`） | 不重试，步骤标 `skipped`，回传「用户未批准」 | 30s 超时 = 拒绝，不二次弹卡 |
| 全局护栏触发（8/6） | 剩余步骤标 `skipped`，`cancel_reason='budget'`，强制收尾 | 与现有强制收尾轮一致 |

### 5.5 L4 中断：`cancel` 标志

- `AgentKit` 增 `canceled: AtomicBool`（或复用现有 `task_id` Mutex 旁挂一个标志），每轮循环开始与每步执行前检查；
- 新增 Tauri 命令 `dialogue_cancel(task_id)`：置标志，前端「停止」按钮调用（**前端按钮是后续增量**，本方案先做后端能力）；
- 用户自然语言「别做了 / 停 / 取消」：在计划轮判定里识别，置 `cancel_reason='user'`，现有循环下一轮前终止，模板收尾；
- 取消后 `task_finish(status="canceled")`，已执行步骤不回滚（read 为主，无副作用；ask 档写操作需用户自己确认过）。

### 5.6 D6 进度面板（只读管理面，独立于消息流）

**数据通道（两条，覆盖面互补）**：

| 通道 | 用途 | 实现 |
|---|---|---|
| 实时 | 执行中的步骤状态推进 | 既有 `task_step` Channel 事件（`task_start`/`task_step`/`task_done`）→ `onDelta.onmessage` 分流 → `planStore` |
| 回放 | 面板首屏 / 切页回来 / 历史计划 | 新增命令 `plan_list(limit)`（复用 `db::task_list` + 解析 `plan_json`）→ `commands.ts` 类型化封装 |

**前端落点（遵循 AGENTS.md 前端约束）**：

- `frontend/src/lib/stores/planStore.ts`（zustand）：`current: PlanProgress | null` + `history: PlanProgress[]` + `applyTaskEvent(v)` + `setHistory(...)`；步骤数组按 `seq` 更新，字段订阅用 selector。
- `frontend/src/lib/types.ts`（或就近领域文件）：`PlanProgress { id, goal, mode, confirmMode, status, steps: PlanStepView[], cancelReason }`——**共享类型集中声明**，不在组件间重复。
- `frontend/src/components/tools/PlanProgressCard.tsx`：插入 `ToolsPane.tsx` 卡片群（顺序在 `ToolCallRecords` 前）；展示目标、进度条（done/total）、步骤列表（intent + status 徽标 + attempts），终态色标。只读，无勾选/编辑。
- `useChat.ts` 的 `onDelta.onmessage`：`task_step` 由「忽略」改为「转 `planStore.applyTaskEvent` 后 return」——**仍不进 `chatStore.messages`**，对话气泡零变化，11-22 裁决的实质（过程不进消息流）不破。
- 慢速轮询兜底：面板挂载时 `plan_list` 拉一次历史；执行中靠事件推进，不做定时轮询（省电、与项目「不打扰」一致）。

**样式**：Tailwind + 现有 CSS 变量，卡片风格对齐 `ToolsPane` 其它卡；status 色复用语义 token（done=success / failed=danger / running=info），不硬编码颜色（AGENTS.md §样式统一）。

**测试**：`planStore.applyTaskEvent` 单测（事件乱序/重复 id 去重/终态对齐）+ `PlanProgressCard` 最小渲染测试，放 `frontend/tests/`，与现有结构一致。

### 5.7 能力线证据出口（预留，不实现）

计划终态可产一条结构化证据（`{goal, steps_done, steps_total, replans, failed}`）——但代码里**没有能力线实体**，本方案只定义数据形状与落点（`tasks` 表已含终态字段），待 G 系列能力线立项时消费。**不得在方案里承诺「已接入能力线」**。

---

## 6. 管线接线一览（改哪些文件、改多少）

| 文件 | 改动 | 量级 |
|---|---|---|
| `memory/db.rs` | `SCHEMA_VERSION` 13→14；4 行 `add_column_if_missing`；`plan_json` 读写 + `plan_list` 扩展单测 | 小 |
| `dialogue.rs` | `plan_intent()`；计划轮 + 特判 `submit_plan` + D1 确认门；状态桥；失败重试；cancel 检查 | 中（核心工作量） |
| `llm/mod.rs` | **不改**（`ToolSpec` / `ToolMsg` / `chat_with_tools` 已够用） | 零 |
| `mcp_admin.rs` | **不改**（`execute_audited` 单点、AgentKit 增一个 cancel 标志） | 极小 |
| `agent_tools.rs` | **不改**（`submit_plan` 是协议工具，不进 provider catalog） | 零 |
| `lib.rs` | `generate_handler!` 增 `plan_list`（D6 回放通道） | 极小 |
| `frontend/src/lib/stores/planStore.ts` | 新增 zustand store（D6） | 小 |
| `frontend/src/lib/commands.ts` + `types.ts` | 新增 `planList` 封装 + `PlanProgress` 类型（D6） | 小 |
| `frontend/src/hooks/useChat.ts` | `task_step` 从忽略改为转 `planStore`（D6，仍不进消息流） | 极小 |
| `frontend/src/components/tools/PlanProgressCard.tsx` + `ToolsPane.tsx` | 新增进度卡并插入卡片群（D6） | 小 |
| `frontend/tests/` | planStore / PlanProgressCard 最小测试（D6） | 小 |

> 后端核心仍是「协议层/执行层/权限层零改动、增量集中在编排逻辑 + schema 列」；**D6 的净新增集中在前端展示面**（1 store + 1 卡 + 1 命令封装），这是用户拍板要做进度面板的代价，不与「能力不重复实现」冲突——面板只消费 `task_step`/`plan_list` 的既有数据，不另写一份状态逻辑。

---

## 7. 触发与预算

| 项 | 值 | 理由 |
|---|---|---|
| 计划轮额外请求 | 命中时 1 次（非流式）；未命中 0 次 | 单步任务零成本，延续「无候选零额外轮次」的设计哲学 |
| 计划步数上限 | ≤6 | 低于工具调用上限 8，留 2 步余量给「计划外步骤」与收尾 |
| 计划内工具调用 | 仍受 ≤8 / ≤6 全局护栏 | 计划不放大预算；超出即跳过剩余步骤并收尾 |
| 重试预算 | 每个 read 步骤 ≤1 次重试 | 防重试风暴；写在 `plan_json.attempts` 可查 |
| replan 次数 | ≤1 | 模型执行中发现计划错误时允许一次重规划，仍写 `tasks.plan_json`（覆盖并留 trace） |

---

## 8. UI 呈现（D6 裁决：做面板，但过程不进消息流）

- **对话气泡**：只显示最终回复 + 🔧 标注（现状不变）。**不显示计划卡、不显示步骤进度**——11-22 裁决的实质保持。
- **工具页（D6 主展示面）**：新增 `PlanProgressCard`——执行中随事件实时推进（目标 / done-total 进度条 / 步骤 intent+status 徽标 / attempts），首屏 `plan_list` 拉最近计划；只读、不可勾选编辑。
- **`/trace` 页（细粒度明细面）**：时间线新增 `plan` 类型步骤卡，展示目标 + 步骤状态 + 实际工具调用关联（`trace_steps.step_kind` 增 `plan_submit` / `step_state` / `retry` 三个枚举值）。
- **两处面板分工**：工具页卡 = 「现在跑到哪」（实时概览），`/trace` = 「每一步为什么」（逐步证据）；两者同源（`tasks.plan_json` + `tool_calls`），不存第三份状态。
- **交付顺序**：后端 T1–T4 可先独立落地（数据进 `tasks` + 事件照发）；T6 面板随后接入，任一时点后端都可单独交付，不被 UI 阻塞。

---

## 9. 分期里程碑（逐可编译单元提交）

用户工作流：每期 `cargo check` + `cargo test --lib` 全绿再进下一期；subject 中文、仅本地、精确路径 `git add`。

| # | 事项 | 交付物 | 门禁 | 依赖 |
|---|---|---|---|---|
| T1 | schema v14 | `db.rs` bump + 4 列（含 D1 `confirm_mode`）+ `plan_json` 读写函数 + 迁移单测（含 v13 老库升级） | `cargo test --lib` 全绿 | 无 |
| T2 | 规划入口与计划轮 | `plan_intent()` + 20 正/15 负 CI 样本；`submit_plan` 协议特判；D1 确认门（auto 直放 + manual 预埋接口）；计划失败降级单测 | 全绿 | T1 |
| T3 | 状态桥 | `cloud_tool_loop` 写回步骤状态；计划外步骤补全；`task_step` 事件形状不变 | 全绿 | T2 |
| T4 | 失败恢复与取消 | 有界重试 / denied 不重试 / 预算跳过；`AgentKit.canceled` + `dialogue_cancel` 命令 | 全绿 | T3 |
| T5 | trace 可读化 | `step_kind` 新增三枚举；`/trace` 时间线计划卡（前端最小改动） | `cargo test` + `tsc --noEmit` + vitest | T4 |
| T6 | **进度面板（D6）** | `plan_list` 命令 + `planStore` + `PlanProgressCard` 插入工具页；`useChat` 事件改转 store（仍不进消息流）+ 单测 | `cargo test` + `tsc --noEmit` + vitest | T3（可并行于 T4/T5） |
| T7 | 真机验收 + 文档 | DeepSeek 真连：计划命中率、步骤-审计对齐、重试有效性、取消响应、面板实时性；回填本方案状态 | 真机清单全过 | T4/T5/T6 |

---

## 10. 验收标准（Given / When / Then 摘要）

| 编号 | 标准 |
|---|---|
| A1 | Given 20 正样本多步目标，When 进对话管线，Then ≥18 个产生 `plan_mode='planned'` 且 `plan_json.steps` 非空 |
| A2 | Given 15 负样本单步问答，When 进管线，Then 0 个产生计划轮（零额外请求，trace 无 `plan_submit` 步骤） |
| A3 | Given 一个 3 步计划且全部成功，When 循环结束，Then `plan_json` 每步 status=done、`tool_call_id` 均指向真实 `tool_calls` 行、任务终态 done |
| A4 | Given 某 read 步骤第 1 次失败第 2 次成功，When 结束，Then 该步 attempts=2、status=done、trace 有一次 retry 记录；写操作失败则 attempts=1 不重试 |
| A5 | Given 用户说「别做了」，When 下一轮循环检查，Then 循环终止、剩余步骤 skipped、`cancel_reason='user'`、任务终态 canceled |
| A6 | Given 计划含 ask 档工具，When 执行到该步，Then 仍逐个弹审批卡，30s 未响应=拒绝=skipped，**无一揽子放行** |
| A7 | Given 计划 6 步但实际调用达 8 次上限，When 护栏触发，Then 剩余步骤 skipped、`cancel_reason='budget'`、强制收尾有正文 |
| A8 | Given 计划轮解析失败/超时，When 降级，Then 走现有无计划循环、用户无感、trace 记 `plan_submit` 失败 |
| A9 | Given 本地模型路径，When 输入多步目标，Then 不产生任何计划、不调用工具、诚实提示需云端（零伪装） |
| A10 | Given 任意计划执行，When 查 `/trace`，Then 步骤状态与 `tool_calls` 审计一一对应，无凭空生成的步骤 |
| A11 | Given `plan_confirm=auto`（默认），When 计划提交成功，Then **零等待**直接进执行循环，无确认卡、无阻塞，每轮只记 `confirm_mode='auto'` |
| A12 | Given `plan_confirm=manual`（扩展位），When 本版计划提交，Then 仍按 auto 放行（本版不实现确认卡），仅在 `tasks.confirm_mode` 留痕——**不得出现「设了 manual 却卡住不动」** |
| A13 | Given 工具页进度面板已挂载，When 一次 3 步计划执行中，Then 面板随 `task_step` 事件实时推进步骤状态（pending→running→done），完成时进度条满 |
| A14 | Given 面板首屏或切页回来，When 调用 `plan_list`，Then 展示最近计划及终态；执行中的计划状态与事件实时态一致（不出现两份相斥的进度） |
| A15 | Given 任意计划执行中，When 观察对话气泡，Then 消息流**只有最终回复**，无任何计划卡/步骤进度文本（11-22 裁决的实质保持，对照 `chatStore.messages`） |

---

## 11. 风险矩阵

| 风险 | 类别 | 概率 | 影响 | 等级 | 应对 |
|---|---|---|---|---|---|
| 计划轮误触发，单步任务被过度规划（成本 + 噪音） | 成本 | 3 | 3 | 9 中 | 规则入口保守判定 + 20/15 CI 样本 + 计划轮失败即降级 |
| 照抄 ReAct 导致第二个执行引擎（违反硬约束） | 架构 | 2 | 5 | 10 高 | 方案硬边界：所有动作单点走 `execute_audited`，计划只产状态；评审时逐行核对 |
| 计划步骤与真实工具调用脱节（进度造假） | 可信 | 3 | 4 | 12 高 | `tool_call_id` 强制关联真实审计行；计划外步骤显式标 `*`；A3/A10 锁定 |
| 重试放大云端成本 / 写操作重复副作用 | 成本/安全 | 2 | 4 | 8 中 | 只重试 read 档 ≤1 次；ask/forbidden/写永不重试 |
| 进度面板侵入消息流（推翻 11-22「过程不进消息流」裁决） | 产品 | 2 | 4 | 8 中 | D6 面板只在工具页 + `/trace`；`useChat` 的 `task_step` 转 `planStore` 后 return，**绝不进 `chatStore.messages`**；A15 单测锁定 |
| 本地路径被间接引入规划（零工具调用失守） | 合规 | 2 | 5 | 10 高 | `plan_intent` 只在云端分支调用；A9 单测锁死；提示词/代码双检查 |
| 取消后已执行的 ask 档写操作无法回滚 | 安全 | 2 | 3 | 6 中 | 取消只拦未执行步骤；已执行写操作的用户已确认过，文档明示「取消不回滚」 |
| 能力线证据出口被误当成已实现 | 范围 | 3 | 2 | 6 中 | §5.7 明确「预留不实现」；文档不承诺接入 |
| 进度面板两份状态相斥（事件实时态 vs `plan_list` 回放态） | 可信 | 3 | 3 | 9 中 | 面板首屏拉 `plan_list` 建基线、执行中事件增量覆盖；终态以 `tasks` 表为准；A14 锁定 |
| `manual` 确认门被误做成真阻塞（本版不该卡住用户） | 范围 | 2 | 3 | 6 中 | 本版 `plan_confirm_gate()` 恒 Allow；A12 单测锁定「设 manual 也不卡」 |

---

## 12. 待裁决决策点

| # | 问题 | 结论 | 备选 / 扩展 | 影响 |
|---|---|---|---|---|
| ~~D1~~ | ~~计划轮是否需要用户确认再执行~~ | **✅ 已裁决（2026-11-23）**：默认确认 = auto 直接执行（先干活）；后续可扩展手动确认 | manual 确认门本版只预埋接口不实现 | 交互节奏与信任模型 |
| D2 | 计划数据结构 | **扩展 `tasks` 表 4 列**（v0.2 加 `confirm_mode`） | 新建 `plans` + `plan_steps` 表（更规范但 schema 更重、违「不重复」） | schema 复杂度 |
| D3 | 步骤状态存几份 | **只存 `plan_json` + 从 `tool_calls` 派生，不双写** | plan_steps 表独立存状态（查询快但两份状态易漂移） | 一致性风险 |
| D4 | 重试上限 | **read 档 1 次** | 0 次（现状，省钱但易失败）；2 次（更稳但更贵） | 成本 vs 可靠性 |
| D5 | `submit_plan` 实现位置 | **协议特判（不进 provider catalog）** | 做成内置工具（统一但要进注册表、语义混淆） | 架构纯度 |
| ~~D6~~ | ~~前端是否要计划进度面板~~ | **✅ 已裁决（2026-11-23）**：做进度面板（工具页只读卡 + `/trace` 明细），**不进对话气泡、不可勾选** | 手动确认卡（随 D1 manual 扩展位一起，后续再议） | 11-22 裁决边界已澄清 |
| D7 | 规划是否跨会话持久 | **不跨**（生命周期=一轮） | 跨会话 todo（需新表 + 新 UI，超出范围） | 范围边界 |

---

## 13. 实施记录

| T | commit | 验证 | 备注 |
|---|---|---|---|
| T1 | `1a7a52f` | `cargo test --lib` 454 passed | schema v14 + 4 列读写；基线 452→454 |
| T2 | `4f4a116` | `cargo test --lib` 462 passed；`plan_intent` 20 正/15 负全过 | 规划入口 + 计划轮 + D1 确认门 |
| T3 | `cdf1ea8` | `cargo test --lib` 464 passed | 步骤状态桥（done/skipped/failed + 审计 id 关联 + 计划外追加）|
| T4 | `010dfba` | `cargo test --lib` 469 passed | 有界重试 + 取消标志 + dialogue_cancel 命令 + 预算/取消收尾 |
| T5 | `d84035f` | `cargo test --lib` 469 passed；`tsc --noEmit` 0 错；`vitest run` 148 passed | `step_state` 记录点 + `/trace` 计划卡（`plan_submit` 目标+步骤、`step_state` 状态+审计 id）；前端类型补三枚举 + 解析容错单测 |
| T6 | `7b27550` | `cargo test --lib` 469 passed；`tsc --noEmit` 0 错；`vitest run` 165 passed | `plan_list` 命令 + `planStore`（回放/实时合流）+ 工具页 `PlanProgressCard` 只读卡；17 个新单测（store 10 / 卡片 5 / 消息流隔离 2）|

**T2 实现与设计的小偏差（不影响约束）**：计划轮在 `chat_with_tools` 的 msgs 尾部追加一条系统指令 user 消息（「只调用 submit_plan」），使模型明确协议；计划 msgs 用 `msgs.clone()`，不污染主循环消息序列。计划轮 HTTP 计入 `CLOUD_REQUEST_CAP`（不放大预算）。任务卡改为「计划成功即开卡」（原惰性开卡太晚，plan_json 需先有归属行），后续真实工具循环见 `task_open=true` 不再重复开卡。`submit_plan` 不落审计行（`execute_audited` 不认识它），只有真实工具审计关联 task。

**T3 实现与设计的小偏差（不影响约束）**：§5.3 原文提到「不在 `tool_hint` 中算计划外」，实装改为**纯按执行序号对齐**（第 N 次执行 → 第 N 步）——`tool_hint` 仅作模型/UI 的意图提示，不强制匹配工具名。理由：小模型换工具是常态，强绑定会让计划步骤永久卡 pending、进度失真；真实工具名通过 `tool_call_id` 关联审计行可查，不双写。只有**执行序号超出计划步数**才算计划外并追加 `tool_hint="*"` 步骤。`execute_audited` 不返回审计行 id，状态桥用「执行前后查 `tool_calls` MAX(id)」关联，只有 id 增大才认（防臆造工具名时误关联上一次）。

**T5 实现要点（不改 schema）**：`step_kind` 是 TEXT 无约束，三个新枚举（`plan_submit`/`step_state`/`retry`）零迁移。`plan_submit`（T2）/`retry`（T4）此前已落库，T5 只补 **`step_state` 记录点**：`bridge_plan_step` 改为返回 `(idx, status, tool_call_id)`，调用点据此记结构化 JSON（`seq/status/attempts/tool_call_id`），前端 `/trace` 计划卡用它把步骤状态与真实审计行关联（A10）。**前端零新增命令**：计划卡消费 `plan_submit.output`（完整 PlanJson），与 T6 的 `plan_list` 命令干净分离。前端 `parseSubmittedPlan`/`parsePlanState` 对坏 JSON 一律回落原文块（不炸）。

**T6 实现要点（D6 落地，不改 schema / 不改事件形状）**：面板两条通道合成一份状态——回放 `plan_list`（`tasks` 表权威终态）+ 实时 Channel 事件（`task_start`/`task_step`/`task_done`/`task_canceled`），全部收敛在 `planStore`，组件只订阅。合并规则写死在 store：**语义取回放**（`intent`/`toolHint` 只有 `plan_json` 有），**状态只升阶**（pending < running < 终态），迟到/重复事件不会把 `done` 打回 `running`，也不会去重成两行（A13/A14）。三处与设计的小偏差：① `plan_list` 落在 `mcp_admin.rs`（命令住在模块里，`lib.rs` 只登记；runbook 原写 `lib.rs`）；② `task_canceled` 事件**不带 id**（后端只发 reason），store 把它归到 `current`；③ `plan_json` 嵌套字段沿落库契约保持 snake_case，前端 `progressFromRow` 显式映射，不靠 serde 改名。此外补了三个语义色 token（`--ok`/`--warn`/`--danger`）到 `base.css`，面板状态色**不散写十六进制**。刷新时机只有两个：`task_start` 与终态事件各拉一次 `plan_list`（补 `intent` 与对齐终态），**不做定时轮询**。A15 由 `useChatPlan.test.tsx` 锁定：同一条流里 `task_*` 只进 `planStore`，`chatStore.messages` 始终只有用户话 + 最终回复。

---

## 14. 变更记录

| 版本 | 日期 | 说明 |
|---|---|---|
| v0.1 | 2026-11-23 | 初稿：认识纠偏（不重造 ReAct）+ 方案骨架；D1/D6 待裁决 |
| v0.2 | 2026-11-23 | 用户裁决落地：**D1** 计划默认确认（auto 直放、manual 扩展位）；**D6** 做进度面板（工具页只读卡 + `/trace`，不进消息流）。新增 §4.1 `confirm_mode` 列、§5.2 D1 确认门、§5.6 进度面板、§6 前端落点、A11–A15 验收、T6 里程碑、相应风险行 |
| v0.4 | 2026-11-24 | P2-12 真机验收通过：规划命中/进度面板/取消/本地零规划抽测过线 |
| v0.3 | 2026-11-24 | P2-10 文首回填：状态改为已实施（T1–T6）；T7 真机验收仍待 |

> T1–T7 已闭合（见 §13）；D1/D6 已裁决；其余 D 点按推荐执行。

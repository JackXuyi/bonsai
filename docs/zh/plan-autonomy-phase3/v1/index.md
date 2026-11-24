# 方案：Phase 3 Autonomy（Task Recovery / 步骤幂等 / Planner·Executor / Action Ranking / 长期主动）

| 文档信息 | 内容 |
|---|---|
| 版本 | v1 |
| 日期 | 2026-11-24 |
| 状态 | **已实施**（2026-11-24；按 §9 推荐默认；`cargo test --lib` 650 绿） |
| 上游输入 | [`roadmap-architecture-r2/v1`](../../roadmap-architecture-r2/v1/index.md) §6；[`plan-planning.md`](../../plan-planning.md)（已落地 T1–T7）；[`plan-reliability-phase1/v1`](../../plan-reliability-phase1/v1/index.md) / [`plan-intelligence-phase2/v1`](../../plan-intelligence-phase2/v1/index.md)；[`roadmap-agent-capabilities.md`](../../roadmap-agent-capabilities.md) P3-5 / P4-1 |
| 定位约束 | Phase 1+2 记忆闭环已稳；本地零工具；能力一份权威；**不引入** Temporal / 多 Agent / Vector；不把 Planner 塞进 Decision |
| 一句话 | **中断可续、写操作可证明不双写、选动作与是否开口分离；常驻 job 不另起调度王国** |

---

## 0. 一句话结论与开工闸

当前是 **Persisted Single-turn Agent**：目标 / Plan / 审计可存、可取消、启动可将过期 `running` 标 `interrupted`——**不是** Durable Agent（无自动续跑、无写步骤幂等证明、Decision 只答「开不开口」）。

Phase 3 升级五条：

1. **A1** Task Recovery：从最后**安全检查点**续跑；区分「工具侧已成功」与「审计/计划状态已提交」。
2. **A2** Step Idempotency：写工具重入策略（跳过 / 查询确认 / 问用户）；无外部副作用证明前**禁止盲目重试**。
3. **A3** Planner 与 Executor 解耦：规则 `plan_intent` + 可选轻量分类器；分类器**只产出计划/候选，不直接执工具**。
4. **A4** Action Candidate Ranking：在 `should_speak` 之上选「做什么」；打扰闸保持独立。
5. **A5** 长期主动任务：与能力 roadmap **P3-5** 统一 job 表对齐，避免平行调度。

**开工闸（满足任一即可开 A1）**：

- Phase 1 L2 全绿且 Forget L3 清单已勾；**或**
- 真实用户/内测出现「崩溃后任务半截、写操作重复」投诉；**或**
- 云端多步计划占比上升且中断/取消率可测。

执行序：**A1+A2（同迭代）→ A3 → A4 → A5**。A5 可与能力线 P3-5 合并立项，禁止本方案另造第二套 cron。

---

## 1. 目标与非目标

### 1.1 目标

| ID | 用户可感知结果 |
|---|---|
| A1 | 进程崩溃或超时中断后，多步任务能从安全步骤续跑，而不是整单作废或 silently 留 `interrupted` |
| A2 | 写文件 / 发提醒类步骤重入时不双写；不确定则问或跳过，不赌 |
| A3 | 「要不要规划」与「执行工具」边界清晰；分类器误判最多浪费一次计划轮，不能越权执行 |
| A4 | 主动场景不只「说或不说」，还能在候选动作里选打扰成本最低且有价值的一项 |
| A5 | 夜间巩固、提醒、日报等走同一 job 语义；失败有界重试、状态可查 |

### 1.2 非目标（本方案明确不做）

| 不做 | 归属 / 理由 |
|---|---|
| Temporal / Cadence 等重型工作流引擎 | roadmap 非目标；本机 SQLite + 有界循环足够 |
| 多 Agent / 子 Agent 集群 | 能力 roadmap「不做」；与一人设冲突 |
| 本地 chat 挂 `tools[]` 做恢复或规划 | AGENTS 硬约束；恢复与执行仍在 Rust / 云端 tool loop |
| 把 Planner 逻辑塞进 `decision.rs` | A4 明确分层：Decision = 打扰闸；Ranking = 选动作 |
| 跨会话无限后台「自由 ReAct」 | 长任务有 job 边界与预算；不做无人值守无限环 |
| Vector / Embedding | 仍属 P5-1 |
| 重写 `cloud_tool_loop` 第二套执行引擎 | 只增强检查点与重入；内核复用 `execute_audited` |
| 一揽子审批绕过 Ask-first | plan-planning A6 维持 |

---

## 2. 现状盘点（Phase 2 之后的代码事实）

| 事实 | 位置 | 对本方案的意义 |
|---|---|---|
| `tasks` + `plan_json` / `plan_mode` / `cancel_reason` / `confirm_mode` | schema v14+；`audit.rs` | 恢复挂载点已有；缺「安全检查点」与 `interrupted` 续跑语义 |
| `PlanStep.status`：`pending\|running\|done\|failed\|skipped` | `PlanStep` | 需补 **`external_effect`** / 幂等键语义（A2），不必新建 steps 表 |
| `tool_calls.task_id` 关联审计 | schema | 「工具成功」证据在审计行；「计划 done」在 `plan_json`——二者可分裂 |
| 启动清残留：`running` 且 `created_at < now-24h` → `interrupted` | `lib.rs` `mark_stale_running_interrupted` | **仅标记**；无 resume API / 无自动续跑 |
| `cloud_tool_loop`：计划轮 → 状态桥 → 有界重试 → cancel | `dialogue/cloud_loop.rs` | 单轮内恢复已有；**跨进程**恢复缺失 |
| `plan_intent` 规则命中 | `cloud_loop.rs` | A3 已有规则腿；分类器可选增强，不得直执工具 |
| `execute_audited` 唯一执行入口 | `mcp_admin.rs` | 重入必须仍走此门；禁止恢复路径旁路 |
| Decision 只产出 `Speak` / `Silent(*)` | `decision.rs` | A4 需**并列** Ranking，不改「是否开口」语义为「做什么」 |
| 主动配额 SQLite 诚实 | Phase 1 R4 | A4/A5 消耗配额仍走 reserve/commit |
| `scheduler.rs` 只服务 consolidate | `memory/scheduler.rs` | A5 触发 P3-5 统一 job；禁止再开第三条轮询线程「各干各的」除非迁入同一 supervisor |
| 日历提醒幂等：`mark_calendar_reminded` | `audit.rs` | A2 写操作范式样板（先查后写） |
| SCHEMA **v19** | Phase 2 I3 | A1/A2 若加列 → **v20**；A5 job 表同版或紧随 |
| 进度面板 / trace 只读 | plan-planning D6 | 恢复进度仍不进对话气泡 |

---

## 3. 不变量（违反即回滚）

| 编号 | 不变量 |
|---|---|
| I1 | **单点执行**：任何续跑 / 重试 / job 触发的工具调用必须经 `execute_audited`（权限 + 审计） |
| I2 | **工具成功 ≠ 审计成功**：仅当审计行落库且 `plan_json` 步骤提交为 `done` 才算安全检查点；分裂时按保守策略处理（见 §4） |
| I3 | **无幂等证明不重放写操作**：write/ask 档步骤默认 `skip` 或 `ask_user`，禁止「失败了再调一次碰运气」 |
| I4 | **本地零工具**：恢复编排、Ranking、job tick 均在 Rust；本地 chat 路径不挂工具 |
| I5 | **Decision ⊥ Planner**：`should_speak` 不选工具、不跑计划；Ranking 不重定义打扰闸 |
| I6 | **分类器不执工具**：A3 分类器最多产出 intent / plan 草稿；执行仍在 loop |
| I7 | **用户取消神圣**：`cancel_reason='user'` 的任务**永不**自动 resume |
| I8 | **预算不放大**：续跑继承原 task 剩余工具/请求预算，不重置 CAP |
| I9 | **调度一份权威**：常驻任务以统一 job 表 + 现有 loop supervisor 为权威；禁止 consolidate / remind / 新 job 各写一套互不知晓的 cron |
| I10 | **过程不进气泡**：恢复与 job 进度走面板 / trace / 事件，不违反 11-22 裁决 |

---

## 4. A1 — Task Recovery（P2）

### 4.1 行为

```text
启动或显式 resume(task_id)
  → 读 tasks 行 + plan_json + tool_calls
  → 若 status ∈ {done, canceled} 或 cancel_reason='user' → 拒绝
  → 若 status = interrupted | running(陈旧) → 进入恢复裁决
  → 计算 last_safe_seq = 最大满足「检查点完整」的 step.seq
  → 将 seq≤last_safe 标 done（若尚未）；seq=last_safe+1 按 A2 裁决
  → 余下 pending 交回 cloud_tool_loop 续跑（同一 task_id，不新建卡）
```

### 4.2 安全检查点定义

一步视为 **checkpoint-complete** 当且仅当：

1. `plan_json.steps[i].status == "done"`；**且**
2. `tool_call_id` 指向存在的 `tool_calls` 行；**且**
3. 该审计行 `outcome` ∈ {`ok`, …产品定义的成功集}（`denied` 对应步骤应为 `skipped`，不算写成功检查点）。

分裂案例：

| 案例 | 裁决（推荐默认） |
|---|---|
| 审计 `ok`，计划仍 `running`/`pending` | **补提交**计划状态为 `done`（信任审计），然后续跑下一步 |
| 计划 `done`，无 `tool_call_id` / 审计缺失 | **降级为未完成**：改回 `pending` 或 `failed`，按 A2 处理（只读可重试，写则 ask/skip） |
| 计划 `running`，无审计 | 视为崩溃在执行中：按 A2（写→ask/skip；读→允许 1 次有界重试） |

### 4.3 API 与触发

| 入口 | 行为 |
|---|---|
| 启动 | 可选：列出 `interrupted` 且 `plan_mode='planned'` 且非 user-cancel；**默认不自动续跑**（D1），只记日志 + 设置/面板「可恢复」标记 |
| `task_resume(task_id)` 命令 | 用户或面板显式触发；云端模式才执行 |
| 对话内「继续刚才的任务」 | 规则命中 → 调同一 resume；本地模式诚实告知需云端 |

推荐默认 **D1**：自动标记可恢复，**不**在启动时静默烧工具配额；避免用户不在场时续跑写操作。

### 4.4 数据模型（schema v20 建议）

在 `tasks` 上增量（`add_column_if_missing`）：

```text
resume_cursor   INTEGER   -- 下一待执行 seq；NULL = 未进入恢复语义
safe_checkpoint TEXT      -- ISO 时间：上次检查点提交成功
interrupt_kind  TEXT      -- 'crash' | 'timeout' | 'budget' | 'external' | NULL
```

`PlanStep` JSON 可增可选字段（向前兼容 serde default）：

```text
idempotency_key: Option<String>   -- A2
effect_class: Option<String>      -- 'read' | 'write' | 'ask' | 'unknown'
```

不新建 `plan_steps` 表（与 plan-planning「不双写」一致）。

---

## 5. A2 — Step Idempotency（P2）

### 5.1 效应分级

| `effect_class` | 来源 | 重入策略 |
|---|---|---|
| `read` | 工具 descriptor 权限档 `read` | 允许有界重试（沿用 plan-planning：≤1） |
| `write` | 权限档隐含写 / 显式标记 | **禁止盲目重试**；见下表 |
| `ask` | 权限 `ask` | 未获批不重放；超时保持 skipped |
| `unknown` | 旧数据 / MCP 未标注 | **等同 write**（保守） |

### 5.2 写操作重入策略表

| 策略 | 何时用 | 行为 |
|---|---|---|
| **S-skip** | 已有成功审计或外部可观测「已完成」 | 步骤标 `done`/`skipped`，不调用工具 |
| **S-query** | 工具提供查询旁路（如「文件是否存在」「提醒是否已标」） | 先只读探测；已完成 → skip；未完成 → 执行一次 |
| **S-ask** | 无法证明 | 暂停任务为 `interrupted` + `interrupt_kind` 留痕；面板提示用户选「重试 / 跳过 / 取消」 |
| **S-forbid** | 明确双写不可接受且无查询 | 标 `failed`，任务失败收尾，不重放 |

推荐默认：

- 内置写工具（文件写、日历 remind 等）优先实现 **S-query**（对齐 `mark_calendar_reminded`）；
- MCP 未知工具 → **S-ask**；
- 无 UI 时启动恢复路径对写步骤 → **S-ask**（不自动 S-forbid 以免丢用户意图，也不自动重放）。

### 5.3 幂等键

- 生成：`idempotency_key = hash(task_id, seq, tool_name, canonical_params)`；
- 执行前查：同 key 已有 `outcome=ok` 审计 → S-skip；
- 写入审计时带上 key（`tool_calls` 增列 `idempotency_key TEXT` 或塞进 `params_summary` 结构化前缀——**推荐独立列**便于索引）。

### 5.4 与单轮内重试的关系

`cloud_tool_loop` 现有「read 失败重试一次」保留；本阶段把 **跨进程重入** 接到同一策略函数，例如：

```text
fn reentry_decision(step, last_audit, effect_class) -> Skip | Retry | AskUser | Fail
```

单测覆盖：写成功审计存在 → Skip；审计无、write → AskUser；read 无审计 → Retry。

---

## 6. A3 — Planner / Executor 解耦（P2）

### 6.1 现状与目标形态

```text
今日：
  plan_intent(规则) → 计划轮(模型) → cloud_tool_loop(执行)

目标：
  IntentGate（规则 ∪ 可选 Classifier）
       → PlanDraft（结构化，可空）
       → ConfirmGate（auto/manual，已有）
       → Executor = cloud_tool_loop + execute_audited
```

| 组件 | 允许 | 禁止 |
|---|---|---|
| IntentGate | 产出 bool / 粗糙类别 | 调工具、改 DB 业务表 |
| Classifier（可选） | 本地小模型或轻量规则打分；仅云端管线启用时参与「是否规划」 | 生成 `tool_calls` 并执行 |
| Planner（计划轮） | 产出 `PlanJson` | 在计划轮内执行副作用工具（`submit_plan` 协议工具除外） |
| Executor | 唯一副作用出口 | 绕过 IntentGate 私自开跑无界循环 |

### 6.2 分类器落地策略（推荐默认 D3）

- **本阶段默认不接模型分类器**；先把边界写成模块（`dialogue/intent_gate.rs` 或 `planning/intent.rs`），把现有 `plan_intent` 迁入。
- 若评测显示规则假阴性 ≥ 阈值，再加可选 Classifier，开关默认 off。
- 分类器输出必须经同一 `PlanJson` schema 校验；失败 → 降级无计划循环（与今日计划轮失败降级一致）。

### 6.3 与「能力一份权威」

IntentGate **不是**日历/文件/记忆的平行实现；只做「要不要进计划外壳」。领域能力仍只在 builtin/MCP 工具内。

---

## 7. A4 — Action Candidate Ranking（P2）

### 7.1 分层

```text
Perception / Calendar / Predictions / Jobs due
        ↓
  CandidateGen（纯函数：列出 ActionCandidate[]）
        ↓
  Rank（价值 − 打扰 − 重复惩罚）
        ↓
  Decision.should_speak / gate_interruptibility + 配额
        ↓
  Speak → 生成层只拿到「已选中的 action」说明书
```

| 层 | 职责 |
|---|---|
| CandidateGen | 「可以做什么」：提醒、预测核对、job 摘要、静默巩固（巩固可不进 Speak） |
| Rank | 「先做哪个」：打分排序，可全员低于阈值 → 空 |
| Decision | 「现在能不能开口」：Busy / idle / 配额；**不**知道工具细节 |
| Generation | 「怎么说」：既有 proactive 生成路径 |

禁止：在 `should_speak` 内硬编码「若日历则提醒」的第二套业务；日历提醒逻辑继续住在权威工具/既有 reminder lane，Ranking 只引用其**候选描述符**。

### 7.2 `ActionCandidate`（建议形状）

```rust
struct ActionCandidate {
    id: String,                 // 稳定 id，便于去重与审计
    kind: ActionKind,           // Remind | PredictVerify | DigestNudge | JobReport | …
    value: f32,
    interruption_cost: f32,
    dedupe_key: String,         // 同日同 key 不重复进候选
    payload: serde_json::Value, // 生成层只读说明书；禁止含密钥
}
```

### 7.3 与配额

- 仅 **需要开口** 的候选在 Decision 过闸后 `try_reserve`；
- 静默 job（consolidate）不走 Speak，不耗主动配额；
- Rank 第一名被 Silent 拒绝时：**不**自动降级到第二名强行说（避免打扰闸被绕过）；可留待下一 tick 再 Rank。

---

## 8. A5 — 长期主动任务（P2，对齐 P3-5）

### 8.1 统一 job 表（建议 v20）

```sql
CREATE TABLE IF NOT EXISTS jobs (
    id            TEXT PRIMARY KEY,
    name          TEXT NOT NULL UNIQUE,  -- 'consolidate' | 'calendar_remind' | …
    schedule_kind TEXT NOT NULL,         -- 'cron_like' | 'interval' | 'once'
    schedule_expr TEXT NOT NULL,         -- 先支持有限方言，勿引入完整 cron 解析库除非必要
    enabled       INTEGER NOT NULL DEFAULT 1,
    last_run_at   TEXT,
    last_status   TEXT,                  -- 'ok' | 'failed' | 'skipped'
    consecutive_fails INTEGER NOT NULL DEFAULT 0,
    payload_json  TEXT NOT NULL DEFAULT '{}'
);
```

### 8.2 迁入策略

| 现有 | 迁入方式 |
|---|---|
| consolidate nightly / catchup | `decide_trigger` 逻辑变为 job 的 schedule 判定；**执行体仍调用** `consolidate::` |
| calendar reminder lane | 保留幂等 `mark_calendar_reminded`；tick 登记为 job |
| 未来日报 / 用户自定义 | 只加 job 行 + 执行器注册表，不新开线程 |

执行器注册表（Rust 内静态）：`name → fn(ctx) -> Result`；**禁止** job 行内嵌脚本引擎。

### 8.3 失败与重试

- 连续失败上限 **≤3**（对齐能力 roadmap P3-5）；达上限 → `enabled` 可保持但 `last_status=failed` + 结构化日志；可选暂停该 job（产品文案另定）。
- 单次 tick **MUST NOT** 阻塞 UI 主线程（AGENTS §11）；沿用 `failure::spawn_guarded` supervisor。

### 8.4 与 A1 的边界

- Job = **调度与周期**；Task/Plan = **一次用户目标的多步执行**。
- Job 触发若需多步工具，可 **派生** 一个 `tasks` 行再走 Executor；崩溃恢复走 A1，不把 job 行本身当成 Plan。

---

## 9. 产品裁决表（推荐默认）

| ID | 问题 | 推荐默认 |
|---|---|---|
| D1 | 启动是否自动 resume？ | **否**；标记可恢复，显式 `task_resume` / 面板触发 |
| D2 | 用户取消的任务？ | **永不**自动 resume |
| D3 | A3 是否上模型分类器？ | **默认不上**；先模块化规则 IntentGate |
| D4 | 写步骤跨进程重入？ | **S-query 优先，否则 S-ask**；禁止盲目重放 |
| D5 | 审计 ok、计划未提交？ | **补提交计划 done**，再续跑 |
| D6 | Rank 第二名在第一名被 Silent 时？ | **本 tick 不说**；不降级强行开口 |
| D7 | schema | A1/A2/A5 列与 jobs 表进 **v20** |
| D8 | PR 拆分 | **PR-A** A1+A2+恢复单测；**PR-B** A3 模块边界；**PR-C** A4 Ranking；**PR-D** A5 jobs（可与能力线 P3-5 合并） |
| D9 | 本地模式遇到「继续任务」 | 诚实提示开云端；零伪装 |
| D10 | 进度展示 | 仍走 PlanProgressCard / trace；不进气泡 |

---

## 10. 里程碑与文件触点

### 10.1 里程碑

| 期 | 内容 | 完成定义 |
|---|---|---|
| **M0** | 检查点纯函数 + 分裂案例单测（无 I/O） | ✅ |
| **M1** | `reentry_decision` + idempotency_key 列；写路径 S-query/S-ask | ✅ |
| **M2** | `task_resume` + 启动可恢复标记；准备态接面板（续跑执行仍须云端对话触发） | ✅ L2；全量 cloud_loop 注入续跑可后续加强 |
| **M3** | IntentGate 模块化（迁 `plan_intent`）；文档化分类器扩展位 | ✅ |
| **M4** | ActionCandidate + Rank 接 proactive tick；Decision 保持纯打扰闸 | ✅ |
| **M5** | jobs 表 + consolidate 迁入；失败 ≤3 | ✅ |
| **M6** | L3 真机：杀进程半截计划 → 面板 resume；写文件不双写 | ⏳ 待真机 |

### 10.2 预期文件触点（最小化）

| 路径 | 变更 |
|---|---|
| `memory/db/schema.rs` | v20：tasks 恢复列、`tool_calls.idempotency_key`、`jobs` |
| `memory/db/audit.rs` | resume 读写、idempotency 查询 |
| `dialogue/cloud_loop.rs` | 续跑入口、检查点提交、重入决策挂接 |
| `dialogue/intent_gate.rs`（或等价） | **新建**：迁入 `plan_intent` |
| `decision.rs` | **尽量不改**打扰语义；Ranking 旁路新模块 |
| `action_rank.rs`（或 `dialogue/proactive_rank.rs`） | **新建**：CandidateGen + Rank |
| `memory/scheduler.rs` / `job.rs` | job 调度与 consolidate 迁入 |
| `lib.rs` | `task_resume`；启动可恢复扫描 |
| `frontend` | 进度卡「恢复」按钮 + commands 封装（D1）；最小单测 |
| 文档 | 本方案；roadmap §6 打勾 / 链接 |

---

## 11. 测试主题（与 A1 同迭代最低集）

| ID | 场景 | 期望 |
|---|---|---|
| T01 | 计划中、工具执行前杀进程 | resume 后从该步按 A2 执行；不跳过未完成步 |
| T02 | 工具成功、审计已写、计划未标 done | 补提交后从下一步续 |
| T03 | 工具成功、审计失败/缺失 | 写→Ask/Skip；读→有界重试 |
| T04 | 超时但外部已完成（S-query 真） | Skip，不双写 |
| T05 | 用户取消 | resume API 拒绝 |
| T06 | 拒绝审批 / 预算耗尽 | 剩余 skipped；resume 不复活被 budget 杀掉的步除非显式新任务 |
| T07 | MCP 中途断开 | 当前步按 unknown/write 保守；任务 interrupted 可恢复 |
| T08 | 写操作重复 resume | 第二次 S-skip 或 S-ask，审计行不增成功写 |
| T09 | IntentGate 负样本 | 单步问答零计划轮 |
| T10 | Rank + Busy | 不 Speak、不耗配额 |
| T11 | job nightly 已跑 | 同本地日不第二次 consolidate |

门禁：`pnpm check`；相关 `cargo test --lib`；改前端则 `tsc` + 最小 vitest。

### 11.1 L3 最小真机清单

1. 云端多步计划执行到中段强制杀 App；
2. 重启后面板可见可恢复任务；
3. 点恢复 → 从中断步继续，最终 done；
4. 含写文件步的任务：恢复后磁盘无双份内容；
5. 用户取消的任务无恢复按钮或点了报错。

---

## 12. 风险

| 风险 | 等级 | 应对 |
|---|---|---|
| 启动自动 resume 在用户不在场写坏数据 | 高 | D1 默认关闭自动；写步骤 S-ask |
| 幂等键参数规范化不一致导致误 skip | 中 | canonical_params 单测；不确定则 Ask |
| Ranking 与 reminder lane 双发 | 中 | 同一 `dedupe_key`；候选只来自权威 lane 的「说明书」 |
| job 表迁移与旧 scheduler 双跑 | 高 | 迁入期单一 tick 入口；特性开关先读 jobs 再 fallback，禁止两路都 fire |
| IntentGate 模块化变「大重构」 | 中 | 只搬函数 + 导出；禁止顺手重写 cloud_loop |
| 范围蔓延成完整 Durable 框架 | 高 | 非目标清单；PR 超范围打回 |

---

## 13. 与上下游文档关系

- 上级：[`roadmap-architecture-r2/v1`](../../roadmap-architecture-r2/v1/index.md) Phase 3。
- 规划已交付：[`plan-planning.md`](../../plan-planning.md)——本方案在其外壳上补**跨进程**恢复与幂等，不重造 ReAct。
- 任务卡：[`plan-task-card.md`](../../plan-task-card.md)——进度不进气泡裁决延续。
- 能力扩展：[`roadmap-agent-capabilities.md`](../../roadmap-agent-capabilities.md) **P3-5** 与本方案 A5 **同一权威**；冲突时合并 PR，不平行实现。
- 前置：Phase 1 Forget/配额、Phase 2 隐式记忆——Autonomy **不**回退其不变量。

---

## 14. 变更记录

| 版本 | 日期 | 说明 |
|---|---|---|
| v1 | 2026-11-24 | 初稿：A1–A5 详细设计、检查点/幂等/分层裁决、里程碑与测试最低集；锚点为 Phase 2 后代码（schema v19、Persisted Single-turn） |
| v1.1 | 2026-11-24 | 按推荐默认实施：schema v20、task_recovery、intent_gate、action_rank、jobs、面板恢复按钮 |

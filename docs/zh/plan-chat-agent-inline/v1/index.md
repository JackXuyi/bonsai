# plan-chat-agent-inline v1 — 聊天内授权 · 预算开关 · 计划进度入聊

| 文档信息 | 内容 |
|---|---|
| 版本 | **v1.3** |
| 日期 | 2026-11-25 |
| 状态 | **已实施**（M0–M3；Q3–Q5 / Q8 已裁决） |
| 上游输入 | `ApprovalCard` / `AskFirstGate` / `cloud_tool_loop` / `planStore` / `PlanProgressCard`；[`plan-planning.md`](../../plan-planning.md)；[`plan-chat-first-ui/v2`](../../plan-chat-first-ui/v2/index.md)；[`spec-s4`](../../specs/agent/spec-s4-approval-audit.md) / [`spec-s6`](../../specs/agent/spec-s6-ui.md) |
| 定位约束 | 本地零工具调用；能力不重复实现；危险动作 Ask-first；信任边界在 Rust；耗时任务不阻塞 UI；聊天优先 |
| 一句话 | 审批改成 Cursor 式聊天内确认；工具/HTTP 预算改成可关开关且默认不限；凡有上限处数字一律 **32**；计划步软顶 32 并在对话流展示进度 |
| 用户裁决 | **2026-11-25**：工具 / HTTP / 计划软顶 / 逃生顶 **一律 32**；授权等待 **`APPROVAL_WAIT = 60s`（1 分钟）**（取代 30s） |

---

## 0. 一句话结论

三件事同一产品方向——**Agent 过程与控制面回到聊天**：

1. **工具授权**：从 `AppShell` 全屏模态（`.mconfirm`）改为聊天区 **inline 授权条**（Cursor 风格：卡在输入框上方 / 消息流底部，不挡整页）。
2. **循环预算**：`CLOUD_TOOL_CALL_CAP` / `CLOUD_REQUEST_CAP` 改为 **settings 开关**，**默认关闭 = 不限制**；打开时上限均为 **32**；「不限制」仍受隐藏逃生顶 **32**（同值，见 §8 Q5）。
3. **计划进度**：取消 `MAX_PLAN_STEPS=6` 硬上限，改为安全软顶 **32**；**推翻** `plan-planning` D6「过程不进对话气泡」，在聊天流内展示计划目标与步骤进度（工具页卡降级为历史/管理面，可保留）。

### 数字一览（已裁决）

| 项 | 值 |
|---|---|
| 开启「限制工具调用」时的上限 `CLOUD_TOOL_CALL_CAP` | **32** |
| 开启「限制 HTTP」时的上限 `CLOUD_REQUEST_CAP` | **32** |
| 开关关闭时的隐藏逃生顶（工具 / HTTP） | **32 / 32** |
| 计划步数安全软顶 | **32** |
| 授权等待 `APPROVAL_WAIT` | **60 秒（1 分钟）** |

后端握手（`ApprovalBroker` + `mcp_approval_reply`）、执行单点（`execute_audited`）、计划状态源（`planStore` + Channel `task_*`）**全部复用**，禁止平行实现。

---

## 1. 现状锚点（实测）

| 现状 | 位置 | 约束 |
|---|---|---|
| 审批 UI = 全屏模态挂在 `AppShell` | `ApprovalCard.tsx` + `AppShell.tsx` | 事件 `tool:approval_required`、回执 `mcp_approval_reply` 可保留；只换呈现与挂载点 |
| Ask 档强制 `AskFirstGate`（现状 30s 超时拒）；`approval_mode` 只影响 Read 档 | `mcp_admin.rs` `APPROVAL_WAIT` / `ApprovalModeRow` | 权限语义不变；等待改为 **60s**；写类仍强制问 |
| Broker **单槽**：同一时刻一个待批 | `ApprovalBroker` | 前端仍单值状态即可；队列不在本期 |
| 工具调用 ≤8、`HTTP` ≤6，硬编码常量；触顶 → `cancel_reason=budget` + 强制收尾 | `cloud_loop.rs` `CLOUD_*_CAP` | 改成可读 settings；常量改为 **32**；关开关时仅逃生顶 32 可 budget |
| 计划步数 ≤6：`submit_plan` schema + `parse_submitted_plan` 双闸 | `MAX_PLAN_STEPS` / `submit_plan_spec` | 改为软顶 **32**（`1..=32`） |
| 计划进度只在工具页 / 设置 · 只读卡；注释写明「过程不进对话气泡」 | `PlanProgressCard.tsx`、`plan-planning` **D6** | **本方案显式推翻 D6**；`planStore` / `task_*` 事件已就绪，前端此前已接 store，缺的是聊天挂载 |
| `useChat` 已把 `task_step` 喂给 `planStore`，**不**进 `chatStore` 气泡 | `useChat.ts` | 新增气泡 kind 或旁路挂件，不把过程当 assistant 正文落库 |
| 消息 kind：`user\|bot\|typing\|prediction\|announcement\|reminder` | `chatStore.ts` | 扩展 `approval` / `plan`（或独立 overlay 组件，见 §4） |
| 配置先例：settings KV，**不 bump schema**（skill_auto 同款） | settings 表 | 预算开关走 KV，SCHEMA 仍 21 |

---

## 2. 目标与非目标

### 2.1 目标

| # | 目标 | 验收直觉 |
|---|---|---|
| G1 | Ask / `approval_mode=ask` 时，确认出现在**聊天框区域**，不再盖住整页 | 用户仍能看见上方消息；焦点可键盘操作 |
| G2 | 回执链路不变：允许 / 拒绝 → `mcp_approval_reply`；**60s** 超时仍拒 | broker / 文案 / 单测同步 60s |
| G3 | 工具次数、HTTP 次数各有开关；**默认关 = 不限制**（仍受逃生顶 32）；开则明示上限 **32** | 默认路径不因「产品开关」提前收尾；极端失控仍在 32 止 |
| G4 | 计划可提交至多 **32** 步（软顶）；聊天内可见 goal + 逐步状态 | 多步任务不必切到工具页 |
| G5 | 过程气泡**不落 `dialogue` 正文**；刷新后可从 `plan_list` / 事件重建或仅会话内有效 | 遵守「结果进消息、过程可观测」 |

### 2.2 非目标

- 不改权限三档、不绕过 `execute_audited`。
- 不做「一揽子批准整份计划里所有写操作」（仍逐步 Ask；可选「本轮此工具」见 Q2）。
- 不做本地模型工具调用 / 本地规划。
- 不新建第二套 ReAct / 审批 broker / 计划状态机。
- 不把工具原始输出刷进聊天正文（明细仍走 `/trace`）。
- 授权等待改为 **60s**；倒计时展示随 inline 条一并做（非另开项）。

---

## 3. 产品形态（对标 Cursor）

### 3.1 聊天内授权（Inline Approval）

```
┌─ 消息流 ──────────────────────────────────┐
│  … 既有气泡 …                              │
│  ┌─ 计划进度（可折叠）─────────────────┐   │
│  │ 目标：…  ·  2/5 · 进行中             │   │
│  │ ① 查日程 ✓  ② 读文件 ▸  ③ …        │   │
│  └─────────────────────────────────────┘   │
└────────────────────────────────────────────┘
┌─ composer-dock ────────────────────────────┐
│  ┌─ 授权条（仅 pending 时）──────────────┐ │
│  │ 允许调用「fs_write」？                  │ │
│  │ 来源 · 本机/远程 · args 摘要            │ │
│  │ [拒绝]  [允许本次]  （可选：本轮此工具） │ │
│  │ ⏱ 1 分钟倒计时                          │ │
│  └─────────────────────────────────────────┘ │
│  ┌─ composer 输入框 ─────────────────────┐ │
│  │ 说点什么…                    [模型][发] │ │
│  └─────────────────────────────────────────┘ │
└────────────────────────────────────────────┘
```

**交互要点**

| 项 | 规格 |
|---|---|
| 位置 | **Composer 上方 sticky 条**（主路径，对标 Cursor 输入区上方确认）；可选同时在流内留一条「待确认」占位，避免滚走后找不到 |
| 模态 | **禁止** `aria-modal` 全屏遮罩；不锁滚动；不抢占整页 |
| 按钮 | 拒绝 / 允许本次；文案贴近现卡 |
| 过期 | **60s** 超时或 id 失效 → 条内「已过期」+「知道了」清除 |
| 可达性 | 出现时焦点落到「允许本次」或容器；Esc = 拒绝（新增，对齐常见快捷键；见 Q1） |
| 挂载 | 从 `AppShell` **挪到** `ChatPane` / `composer-dock`；设置页等非聊天路由若触发 Ask，仍应能看到条——推荐：条挂在 `AppShell` 但 **样式/位置锚定聊天 dock**（有 ChatPane 时），无聊天时降级为非模态底部 toast 条 |

### 3.2 计划进度入聊

| 项 | 规格 |
|---|---|
| 出现时机 | `task_start` + `planned: true` → 插入/更新流内 `PlanInline` |
| 内容 | goal、done/total、每步 intent + 状态 chip；`cancel_reason` / `interrupt_kind` 一行 |
| 更新 | 订阅 `planStore.current`（已由 `useChat` 喂事件），**不**轮询 |
| 折叠 | 默认展开进行中；`done`/`canceled`/`failed` 后自动折叠为「计划 · N/N · 状态」一行 |
| 恢复 | `resumable` 时在 inline 卡底部露出「准备恢复 / 重试 / 跳过 / 取消」（从 `PlanProgressCard` 抽共享子组件） |
| 落库 | **不写** assistant 气泡正文；会话重载：`plan_list` 若仍有 running/interrupted 可再挂；已结束计划不强制回放进历史气泡（避免刷屏） |
| 工具页 | `PlanProgressCard` 保留作管理/历史；副文案改为「完整历史在此；进行中也可在对话里看」 |

### 3.3 预算开关（设置）

放在 Agent / 工具相关设置区（邻接 `ApprovalModeRow`）：

| 开关 | settings key | 默认 | 开启时行为 |
|---|---|---|---|
| 限制工具调用次数 | `cloud_tool_call_cap_enabled` | **`false`** | `executed >= 32` → budget 收尾（`CLOUD_TOOL_CALL_CAP = 32`） |
| 限制云端 HTTP 次数 | `cloud_request_cap_enabled` | **`false`** | `requests` 触顶逻辑同现网，上限 **32**（`CLOUD_REQUEST_CAP = 32`） |

说明文案：关闭时「不主动限制（仍有安全上限 32）」；开启时「上限 32 次」。数值本期固定 **32**，不做自由输入（Q4 已裁决）。

> 注：开启上限与关闭时逃生顶同为 32 时，**触顶次数相同**；开关差在产品语义与文案（开启 = 用户知情的预算；关闭 = 仅防失控的安全网，收尾提示用「安全上限」而非「已达你设置的限制」）。为后续可调数字预留开关位。

---

## 4. 架构与改动面

### 4.1 总览

```
                    ┌─ settings KV ─────────────────────────┐
                    │ approval_mode                         │
                    │ cloud_tool_call_cap_enabled (新)      │
                    │ cloud_request_cap_enabled (新)        │
                    └──────────────┬────────────────────────┘
                                   │ build_agent_kit / cloud_tool_loop 读
┌─ Rust ─────────────────────────────────────────────────────┐
│ AskFirstGate ──emit──► tool:approval_required               │
│          ▲                                                  │
│          └── mcp_approval_reply ◄── 前端 inline 条           │
│ cloud_tool_loop：按开关决定是否 enforce CAP；计划解析无步数硬拒 │
│ Channel task_* 不变                                         │
└────────────────────────────────────────────────────────────┘
┌─ 前端 ─────────────────────────────────────────────────────┐
│ ChatPane                                                    │
│   MessageList + PlanInline（← planStore）                   │
│   composer-dock: InlineApprovalBar + Composer               │
│ AppShell：移除全屏 ApprovalCard（或改成 null 壳）            │
│ Settings：ApprovalModeRow 旁 + BudgetCapsRow（新）          │
│ 共享：PlanStepList（从 PlanProgressCard 抽出）               │
└────────────────────────────────────────────────────────────┘
```

### 4.2 前端：授权

| 文件 | 动作 |
|---|---|
| `ApprovalCard.tsx` | 重命名/改造为 `InlineApprovalBar.tsx`：去 `.mconfirm` 遮罩；布局进 `composer-dock`；倒计时按 **60s** |
| `AppShell.tsx` | 卸下模态；若全局挂载则改为渲染 `InlineApprovalBar` 的 portal 目标，或仅在 `ChatPane` 挂 |
| `Composer.tsx` / `ChatPane.tsx` | dock 结构预留授权槽 |
| `ApprovalModeRow.tsx` | 文案：「弹确认卡」→「在聊天里确认」；「30 秒」→「1 分钟」 |
| `mcp_admin.rs` | `APPROVAL_WAIT`：`from_secs(30)` → **`from_secs(60)`**；相关单测/注释同步 |
| 测试 | `ApprovalCard.test.tsx` → 断言无 `aria-modal` 全屏；有 dock testid；文案含 1 分钟 |

**事件契约不变**（字段可加、不可删）：

```json
{ "id": 1, "tool": "fs_write", "source": "builtin", "remote": false, "argsSummary": "…" }
```

可选增量（向后兼容）：`taskId`、`access: "ask"|"read"`，便于条上展示「写操作」。

### 4.3 前端：计划入聊

| 文件 | 动作 |
|---|---|
| `chatStore.ts` | 新增 kind **`plan`**（或独立 `useChatPlanInline` 不进 messages——推荐 **进 messages 列表但 kind=plan**，滚动与贴底自然） |
| `MessageBubble.tsx` 或新 `PlanInlineBubble.tsx` | 渲染；复用 `PlanDetail` 抽出来的展示 |
| `useChat.ts` | `task_start(planned)` → `store.add({ kind:'plan', planId })`；后续只靠 `planStore` 刷新同一气泡（`patch` 或由气泡订阅 store） |
| `PlanProgressCard.tsx` | 抽 `PlanStepsView`；卡本身变薄 |
| 测试 | 扩展 `useChatPlan` / 新 `PlanInline.test.tsx`：事件推进 chip；完成后折叠 |

**推荐实现细节（最小状态）**

- 气泡只存 `planTaskId: string`；UI `usePlanStore(s => s.current?.id === planTaskId ? s.current : …)`。
- 避免把每步 status 复制进 chatStore（单一事实源 = `planStore`）。

### 4.4 Rust：预算开关

```text
fn tool_cap_enabled(conn) -> bool  // settings cloud_tool_call_cap_enabled == "true"
fn request_cap_enabled(conn) -> bool
```

在 `cloud_tool_loop` 护栏处：

```rust
let budget_hit = (tool_cap_on && executed >= CLOUD_TOOL_CALL_CAP)
    || (request_cap_on && requests + 1 >= CLOUD_REQUEST_CAP);
```

- 轮层 retry 的 `requests < CLOUD_REQUEST_CAP`：关闭 request cap 时改为「允许 retry」（仍受下方 **逃生顶** 约束，见 Q5）。
- 读 settings：**每轮开始读一次**（或 kit 构建时打进 `AgentKit`），避免持锁循环内反复读；失败默认 **false（不限制）**——与「默认不限」一致，且校验失败不可静默变成收紧。
- 新命令或并入现有 settings get/set：`agent_budget_get` / `agent_budget_set`（或通用 `settings_get` 若已有）；前端 `BudgetCapsRow`。
- **SCHEMA 不 bump**；仅 KV。

### 4.5 Rust：计划步数解限

| 点 | 改动 |
|---|---|
| `MAX_PLAN_STEPS` | 改为 **`SOFT_PLAN_STEPS_CAP = 32`**：超 32 → 拒计划降级无计划循环（或 truncate 前 32，实施选拒更简单、更诚实） |
| `submit_plan_spec` | description / schema 改为「最多 32 步」；强调 intent 非空 |
| `parse_submitted_plan` | 校验：goal 非空、steps **1..=32**、每步 intent 非空；`len > 32 → None` |
| 计划外步骤 / 状态桥 | 仍按执行序号对齐；步数变多时 UI 折叠，逻辑不变 |
| 单测 | A7 等预算用例改为触顶 **32** + **cap 开启**；新增「cap 关闭时仅逃生顶 32」；新增「32 步可解析、33 步拒」 |

### 4.6 文档与裁决修订

| 文档 | 动作 |
|---|---|
| 本文件 | 权威方案 |
| `plan-planning.md` | 文首加「D6 已被 plan-chat-agent-inline v1 推翻」指针（不改写整份历史） |
| `spec-s6-ui.md` | 审批 UI 描述改为 inline |
| `chatStore` 头注释 | 删除「工具过程永不进消息流」绝对表述 → 「工具原文不进；授权条与计划进度可进」 |

---

## 5. 安全与工程约束对齐

| 约束 | 本方案 |
|---|---|
| 信任边界 | 批准只经 `mcp_approval_reply` + broker id；前端伪造按钮无效 |
| Ask 覆盖率 | `ToolAccess::Ask` 仍无视 `approval_mode` |
| 默认上传 | 无 |
| 长任务不堵 UI | Ask 仍阻塞**该对话轮的 Rust 执行线程**（既有行为）；UI 不模态冻结；用户可看消息、可点拒绝 |
| 变更最小 | 不新依赖；不新表；不第二套审批 |
| 质量门 | Rust：`cloud_loop` / approval / parse_plan 单测；前端：Approval inline + Plan inline + Budget row；`tsc` / lint |

---

## 6. 分期

| 期 | 交付 | 依赖 |
|---|---|---|
| **M0** | Inline 授权条替换全屏卡；`APPROVAL_WAIT=60s`；文案/测试/a11y | 无 |
| **M1** | 预算双开关 + loop 读取 + 设置行 + 单测 | 无（可与 M0 并行） |
| **M2** | 去掉计划步数硬限；`submit_plan` / parse / 单测 | 无 |
| **M3** | 聊天内 `PlanInline` + 抽共享步骤视图；工具页卡降级文案 | M2 更佳（长计划才明显）但可先做展示 |

推荐实施序：**M0 → M1 → M2 → M3**（授权体感优先；预算默认解开避免长任务误杀；再解计划与入聊）。

---

## 7. 测试计划

### 7.1 自动化

| ID | 场景 |
|---|---|
| T1 | Inline 条收到事件渲染；允许 → `mcp_approval_reply(true)`；拒绝同理 |
| T2 | 过期回执 → 过期态，无二次成功 reply |
| T3 | cap 双关：模拟 executed 很大仍不 `budget`（单测注入开关） |
| T4 | 仅 tool cap 开：第 32 次后 force_answer + cancel_reason budget |
| T5 | `parse_submitted_plan` 接受 32 步；33 步 / 空 steps / 空 intent 仍拒 |
| T6 | PlanInline：task_start → 出现；task_step → chip；task_done → 折叠 |
| T7 | 回归：Ask 档在 auto 模式下仍走 ask_gate |

### 7.2 真机

- [ ] `approval_mode=ask`：只读工具在聊天 dock 确认，无全屏遮罩
- [ ] `fs_write`：条出现；允许后写入成功；拒绝后模型收到 denied
- [ ] 默认设置下连续工具可跑到接近 32；第 32 次触发安全上限收尾
- [ ] 打开双开关后同样在 32 收尾，文案为产品「已达限制」而非「安全上限」
- [ ] 模型提交 32 步计划：聊天内可折叠列出并推进；33 步降级
- [ ] Esc / 超时拒绝可感知

---

## 8. 裁决（Q1–Q7）

| ID | 问题 | 结论 | 状态 |
|---|---|---|---|
| **Q1** | Esc 是否 = 拒绝？ | **是** | 推荐，待异议 |
| **Q2** | 是否增加「本轮允许此工具」？ | **本期不做** | 推荐，待异议 |
| **Q8** | 授权等待多久？ | **`APPROVAL_WAIT = 60s`（1 分钟）** | **已裁决 2026-11-25** |
| **Q3** | 计划步数上限？ | **软顶 32**（`1..=32`，超则拒计划） | **已裁决 2026-11-25** |
| **Q4** | 开启 cap 后数字？ | **固定工具 32 / HTTP 32**（布尔开关，无数字框） | **已裁决 2026-11-25** |
| **Q5** | 「不限制」是否仍要逃生顶？ | **要**：隐藏硬顶 **工具 32 / HTTP 32**；不进设置 UI；收尾提示「安全上限」 | **已裁决 2026-11-25** |
| **Q6** | 已结束计划是否写入历史气泡？ | **不写**；仅进行中/本轮展示 | 推荐，待异议 |
| **Q7** | 非 Chat 路由触发审批？ | **AppShell 挂条 + 自动切回聊天或底部非模态条** | 推荐，待异议 |

常量落地名建议：`CLOUD_TOOL_CALL_CAP = 32`、`CLOUD_REQUEST_CAP = 32`、`SOFT_PLAN_STEPS_CAP = 32`、`CLOUD_*_ESCAPE_CAP = 32`（与产品 cap 同值时可共用同一常量，开关只影响文案/`cancel_reason` 细分）。

---

## 9. 风险

| 风险 | 缓解 |
|---|---|
| 长循环费 token / 卡死 | 逃生顶 32；Composer「停」取消；UI 进度可见 |
| 计划步多刷屏 | 默认折叠历史步；只高亮当前 + 前后 1；硬顶 32 |
| D6 用户曾要「不进气泡」 | 本需求显式推翻；过程仍不进 **assistant 正文 / DB 对话行** |
| 审批条与键盘输入抢焦点 | 条出现 focus 容器；关闭后 focus 回 textarea |
| settings 读失败变收紧 | 失败默认不限制 + 日志 |

---

## 10. 变更记录

| 版本 | 日期 | 说明 |
|---|---|---|
| v1 | 2026-11-25 | 初稿 |
| v1.1 | 2026-11-25 | 用户裁决：工具 / HTTP / 计划软顶 / 逃生顶 **一律 32**（替换 8·6·64·48） |
| v1.2 | 2026-11-25 | 用户裁决：授权等待 **30s → 60s（1 分钟）** |
| v1.3 | 2026-11-25 | **已实施** M0–M3：InlineApprovalBar、预算开关、软顶 32、PlanInline |

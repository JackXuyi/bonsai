# plan-message-trace — 消息操作条与链路追踪详情页（v0.2 已裁决）

> 状态：**已确认**（D1：全量永久保留；D2：时间常驻 + 复制/详情 hover 浮现）。
> 详细 spec：docs/zh/specs/agent/spec-s8-message-trace.md
> 日期：2026-11-20

## 1. 目标

1. 消息气泡底部新增操作条：**时间、复制、查看详情**。
2. 「查看详情」进入新页面 `/trace/[turnId]`，以**链路追踪（trace timeline）**方式展示该回复的生成全过程：
   每一步 tool 调用与返回值、每一轮 LLM 的 prompt 与输出、各阶段耗时。

## 2. 现状事实（代码核查，2026-11-20）

| 事实 | 位置 |
|---|---|
| 气泡只有正文 + badge，无时间/复制/详情 | `frontend/src/components/chat/MessageBubble.tsx` |
| `ChatMessage` 无 `ts` 字段（历史回放只把日期塞进 note） | `frontend/src/lib/stores/chatStore.ts` |
| 消息 = `events` 表 dialogue 行，user/assistant 成对落库，**无 turn 关联 id** | `src-tauri/src/memory/db.rs` `add_event_meta` |
| `dialogue_history` 返回 `{ts, role, content}`，**不带 event id** | `db.rs:1552`、`ChatPane.tsx` |
| `tool_calls` 表有 `prompt_trace`，但与具体回合脱钩 | `db.rs:222` |
| `agent_pipeline` 三段式（pass1 候选 → pass2 决策 → pass3 总结）+ JSON 泄漏重试，`kit.trace()` 只在内存，**LLM 每轮 prompt/输出不留痕** | `src-tauri/src/dialogue.rs:182-434` |
| 管线顺序：危机拦截 → 日历规则路由 → 模型 → 反谄媚过滤 → 落库 | `chat_turn_inner` `dialogue.rs:504` |
| 路由为 Next.js app router（`/`、`/face`、`/memory`、`/tools`） | `frontend/src/app/` |

## 3. 数据层（Rust，schema v9）

> 修正：当前 `SCHEMA_VERSION = 8`（spec-s2 mcp_servers 已占用 v8），本轮新表升 **v9**。

### 3.1 新表

```sql
-- 回合主档：一次 chat_turn 一行（含 crisis / 工具命中 / 纯模型）
CREATE TABLE turn_trace (
  id            INTEGER PRIMARY KEY AUTOINCREMENT,
  session_id    TEXT NOT NULL,
  user_event_id INTEGER,          -- 回填 events.id（crisis/工具路径同样成对落库）
  assistant_event_id INTEGER,
  kind          TEXT NOT NULL,    -- model | tool | crisis | proactive
  model_used    INTEGER NOT NULL DEFAULT 0,
  tool_label    TEXT,             -- builtin:x / mcp:x / skill:x
  filter_report TEXT,             -- 反谄媚过滤 JSON（explicit/hedged）
  duration_ms   INTEGER,
  ts            TEXT NOT NULL
);
CREATE INDEX idx_turn_trace_event ON turn_trace(user_event_id, assistant_event_id);

-- 步骤明细：每一步一行
CREATE TABLE trace_steps (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  turn_id     INTEGER NOT NULL REFERENCES turn_trace(id),
  seq         INTEGER NOT NULL,
  step_kind   TEXT NOT NULL,   -- crisis_check | tool_intent | llm_round | tool_call | json_retry | prose_retry | filter
  title       TEXT NOT NULL,
  prompt      TEXT,            -- LLM 轮的系统+用户输入（截断见 3.2）
  output      TEXT,            -- LLM 原始输出
  args_json   TEXT,            -- 工具参数
  summary     TEXT,            -- 工具返回摘要（ToolOutput.summary）
  outcome     TEXT,            -- ok | denied | error | miss | hit | pass
  duration_ms INTEGER,
  ts          TEXT NOT NULL
);
```

要点：
- `turn_trace` 独立于 `events`，**不进 `ACTIVITY_KINDS`**（不产生形态/关系/休眠副作用，遵守 2026-11-20 踩坑铁律）。
- 工具调用继续走 `execute_audited` 的 `tool_calls` 审计（职责不变），`trace_steps` 只冗余一份**面向展示**的轻量记录（args/summary/outcome/duration）。
- agent_pipeline 各分支全部埋点：pass1/pas2/pass3 的 prompt+输出、`level1:no-valid-candidate`、`level2:json-not-prose`（prose_retry）、`level3:json-not-prose`（json_retry）、工具调用结果。
- 流式轮：`output` 记最终拼接全文；`prompt` 记 sys2 全文。
- turn 结束时把 user/assistant 两行 `events.id` 回填 `turn_trace`（成对插入后再 UPDATE）。

### 3.2 容量与保留（D1 已裁决）

- prompt / output **全量存储，不做截断**；**无 prune、无 TTL、无行数上限**（全量永久保留）。理由：链路追踪是排障与信任产品，截断等于丢证据；体积问题留给 P2-5 数据可携带一并裁决。
- 仅存本地 SQLite，不上传（本地优先原则不变）。

### 3.3 核查修正（2026-11-20 实读确认）

- `dialogue_bubbles`（db.rs:1596）**已返回 event `id`**，前端 `bubblesFromHistory` 未接——历史气泡携带 trace 入口零后端改动。
- `dialogue_chat` / `dialogue_chat_stream` 直接返回 `DialogueOutcome`——`turn_id` 加字段即随命令下发，无需新事件。

## 4. 命令层

| 命令 | 说明 |
|---|---|
| `turn_trace_get(turn_id)` | 返回 `TurnTraceDetail { 主档 + steps[] }` |
| `turn_trace_find_by_event(event_id)` | 详情页兜底入口：历史气泡只有 event id 时反查 turn（user/assistant 任一命中均可） |
| ~~`dialogue_history` 加 id~~ | **无需改**：`dialogue_bubbles` 已返回 `id`（db.rs:1596），前端接上即可 |
| DialogueOutcome 加 `turn_id` | dialogue_chat / dialogue_chat_stream 返回值直接下发，实时气泡零额外通道 |

## 5. 前端

### 5.1 气泡操作条（设计稿 1）

- `ChatMessage` 增加字段：`ts?: string`、`eventId?: number`、`turnId?: number`。
- 新组件 `MessageActions.tsx`（`src/components/chat/`，私有组件就近放置）：
  - 时间：今天 `HH:MM`，昨天 `昨天 HH:MM`，更早 `M-D HH:MM`（与现有 note 日期规则统一）。
  - 复制：`navigator.clipboard.writeText` + toast「已复制」。
  - 查看详情：仅当该消息有 trace（`turnId` 可查）时显示，`router.push(/trace/{turnId})`；旧消息（turn_trace 上线前）不显示。
  - 交互：时间常驻灰字；复制/详情 hover 浮现（`opacity` 过渡，键盘可达）。
- 流式中不渲染操作条（`streaming` 时隐藏）。

### 5.2 追踪详情页（设计稿 2）— `frontend/src/app/trace/[turnId]/page.tsx`

结构（自上而下）：

1. **回合摘要卡**：kind 徽标（模型回合/工具回合/危机拦截/主动搭话）、模型名、总耗时、过滤结果；用户输入 + 最终回复（secondary/primary 双色区分）。
2. **垂直时间线**：`trace_steps` 按 seq 渲染；每步一卡：
   - 节点色：灰=规则检查，蓝=LLM 轮，绿=成功工具/落库，红=失败/拒绝，橙=重试。
   - 标题 + 右侧 outcome 与耗时；`llm_round` 显示输出首行摘要（token 数不做精确统计，留后扩展）。
   - **展开交互**：LLM 轮展开看 prompt 全文（等宽、pre-wrap、可复制）；tool_call 展开看请求参数 JSON + 返回摘要。
3. **空态**：查不到 trace → 「该消息产生于链路追踪上线之前，没有留下记录」。
4. 顶部「← 返回对话」（`router.back()`）。

### 5.3 约束遵守（AGENTS.md）

- 操作条抽成 `MessageActions`，单文件 <150 行；追踪页时间线节点 >80 行 JSX 时拆 `TraceStepCard` 子组件。
- 样式跟随现状（globals.css 语义 class / CSS 变量），不新建零散 CSS 文件、不硬编码颜色。
- Tauri invoke 封装进 `src/lib/commands.ts`，不散落叶子组件。

## 6. 里程碑（已并入 spec-s8 分段 S8-a~d）

| 里程碑 | 内容 | 验收 |
|---|---|---|
| M1 = S8-a | schema v9（turn_trace + trace_steps）+ db helper + TraceRecorder + 全路径埋点 + DialogueOutcome.turn_id + 单测 | cargo test 全绿（新增 ~10）；三条路径都有 trace 行 |
| M2 = S8-b | turn_trace_get / find_by_event 命令 + 前端 types/commands | cargo + tsc |
| M3 = S8-c | ChatMessage 字段 + MessageActions（时间/复制/详情）+ useChat / useAppEvents / ChatPane 接线 | vitest 绿；流式不显示操作条；旧消息懒查反查 |
| M4 = S8-d | /trace/[turnId] 追踪页：摘要卡 + 时间线 + 展开 + 空态 + ?event 反查 | 手动验收三条路径；展开 prompt/返回值正确 |
| M5 | globals.css 操作条样式收尾 + 真机验收清单过一遍 | cargo + vitest + tsc 全绿 |

## 7. 不做（本轮边界）

- 不做 trace 的检索/筛选页（只有单回合详情）。
- 不做跨回合的「会话级 trace 聚合视图」。
- 不改动 `tool_calls` 审计语义；不做 token 精确统计（本地 Ollama API 无稳定计数，用长度近似展示或不展示）。

## 8. 决策点（已裁决 2026-11-20）

- **D1 存储粒度与保留**：✅ **全量永久保留**——prompt/output 不截断、无 prune/TTL/行数上限。
- **D2 操作条交互**：✅ **时间常驻 + 复制/详情 hover 浮现**（`.bubble:hover` / focus-within 控制透明度，键盘可达）。

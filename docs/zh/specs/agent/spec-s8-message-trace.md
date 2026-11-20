# spec-s8 — 消息操作条 + 链路追踪详情页（schema v9）

> 依赖：无（独立于 s1–s7，但复用 tool_calls 审计与 agent pipeline 现状）。
> 方案稿：docs/zh/plan-message-trace.md（D1 已裁决：**全量永久保留**；D2 已裁决：**时间常驻 + 复制/详情 hover 浮现**）。
> 命名沿 agent 系列：本 spec = S8，分四段交付 S8-a 数据层 / S8-b 命令层 / S8-c 气泡操作条 / S8-d 追踪页。

## 0. 现状锚点（实施时对齐，勿凭记忆）

| 事实 | 位置 |
|---|---|
| `SCHEMA_VERSION = 8`（s2 已占用） | `memory/db.rs:20` → **本轮升 9** |
| `add_event_meta` 返回事件 rowid | `db.rs:358` |
| `dialogue_bubbles` 已返回 `id`，前端 `bubblesFromHistory` 未接 | `db.rs:1596`、`ChatPane.tsx:20` |
| `DialogueOutcome { reply, filter, model_used, crisis, tool }` 直接回传前端 | `dialogue.rs:47`、`dialogue_chat_stream` `dialogue.rs:805` |
| proactive 事件载荷 `{ text }` | `useAppEvents.ts:50` |
| agent_pipeline 三段式 + 两次 JSON 泄漏重试 | `dialogue.rs:182-434` |
| 前端路由 Next.js app router；消息组件在 `src/components/chat/` | `frontend/src/app/` |

## 1. schema v9（memory/db.rs，S8-a）

- `SCHEMA_VERSION = 9`；migrate 追加（幂等，`CREATE TABLE IF NOT EXISTS` 惯例）：

```sql
CREATE TABLE IF NOT EXISTS turn_trace (
  id                 INTEGER PRIMARY KEY AUTOINCREMENT,
  session_id         TEXT NOT NULL,
  user_event_id      INTEGER,          -- crisis/tool/model 路径都成对落库，回填 events.id
  assistant_event_id INTEGER,          -- proactive 路径为 NULL（只有 assistant 行）
  kind               TEXT NOT NULL,    -- 'model' | 'tool' | 'crisis' | 'proactive'
  model_used         INTEGER NOT NULL DEFAULT 0,
  tool_label         TEXT,             -- 'builtin:x' | 'mcp:x' | 'skill:x'，沿用 DialogueOutcome.tool 语义
  filter_report      TEXT,             -- '{"explicit":bool,"hedged":bool}' JSON
  duration_ms        INTEGER,          -- 回合总耗时（Instant 起点 = chat_turn_inner 入口）
  ts                 TEXT NOT NULL     -- clock::now_iso()
);
CREATE INDEX IF NOT EXISTS idx_turn_trace_user   ON turn_trace(user_event_id);
CREATE INDEX IF NOT EXISTS idx_turn_trace_asst   ON turn_trace(assistant_event_id);

CREATE TABLE IF NOT EXISTS trace_steps (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  turn_id     INTEGER NOT NULL REFERENCES turn_trace(id) ON DELETE CASCADE,
  seq         INTEGER NOT NULL,        -- 1 起步，时间线顺序
  step_kind   TEXT NOT NULL,           -- 'crisis_check' | 'tool_intent' | 'llm_round' | 'tool_call' | 'json_retry' | 'prose_retry' | 'filter'
  title       TEXT NOT NULL,           -- 面向展示的中文标题，见 §3 埋点表
  prompt      TEXT,                    -- LLM 轮：本轮完整 system（含注入段）；其他为 NULL
  output      TEXT,                    -- LLM 轮：模型原始输出全文；工具轮：回复文本
  args_json   TEXT,                    -- tool_call：参数 JSON 原文
  summary     TEXT,                    -- tool_call：ToolOutput.summary
  outcome     TEXT,                    -- 'ok'|'denied'|'error'|'hit'|'miss'|'pass'|'answered'|'direct'|'none'
  duration_ms INTEGER,
  ts          TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_trace_steps_turn ON trace_steps(turn_id, seq);
```

- **D1 落地**：prompt / output **全量存储，不做任何截断**；**无 prune、无 TTL、无行数上限**（全量永久保留，本机 SQLite 自担体积）。代码注释记一笔 deviation rationale：链路追踪是排障与信任产品，截断等于丢证据。
- `turn_trace` / `trace_steps` **不进 `ACTIVITY_KINDS`**、不产生任何派生事件（2026-11-20 踩坑铁律：trace 是旁路记录，不是 activity）。

## 2. db 层 helper（db.rs）

```rust
pub struct TraceStepDraft { /* 与 trace_steps 列一一对应，duration_ms 由调用方算好 */ }
pub struct TraceTurnDraft { session_id, kind, model_used, tool_label, filter_json, duration_ms, steps: Vec<TraceStepDraft> }
```

| 函数 | 行为 |
|---|---|
| `trace_turn_persist(conn, &TraceTurnDraft, user_event_id: Option<i64>, assistant_event_id: Option<i64>) -> rusqlite::Result<i64>` | 单锁内一次性写 turn 主档 + 全部 steps（一个事务），返回 turn_id；**在事件插入的同一 lock scope 内调用** |
| `turn_trace_get(conn, turn_id) -> rusqlite::Result<Option<(TraceTurnRow, Vec<TraceStepRow>)>>` | 详情页主查询；turn 不存在返回 None（前端空态） |
| `turn_trace_find_by_event(conn, event_id) -> rusqlite::Result<Option<i64>>` | 气泡 event id 反查 turn_id：`user_event_id = ?1 OR assistant_event_id = ?1`，命中即返回 |

- 序列化结构 `TraceTurnRow` / `TraceStepRow` 直接 `Serialize`（snake_case 字段，前端 types 对齐）。

## 3. 埋点（dialogue.rs，S8-a 核心）

### 3.1 收集器

```rust
/// 回合内 trace 收集器：内存累积，回合末一次性落库（单锁单事务）。
/// 起点即 chat_turn_inner 入口，duration_ms 全程 Instant::now 差值。
pub(crate) struct TraceRecorder {
    started: std::time::Instant,
    steps: Vec<TraceStepDraft>,
}
impl TraceRecorder {
    fn new() -> Self;
    fn llm_round(&mut self, title: &str, prompt: &str, output: &str, outcome: &str, dur: Duration);
    fn tool_call(&mut self, name: &str, args_json: &str, summary: &str, outcome: &str, dur: Duration);
    fn simple(&mut self, step_kind: &str, title: &str, outcome: &str, output: Option<&str>);
}
```

- `chat_turn_inner` 入口 `let rec = TraceRecorder::new();`；`agent_pipeline` 签名增加 `rec: &mut TraceRecorder` 参数（现有测试机械适配——mock backend 的 reply 本来就会被记录，断言不受影响）。

### 3.2 埋点位置与 step 内容（逐路径）

| 路径 | step_kind / title | 记录内容 | outcome |
|---|---|---|---|
| 危机拦截（`crisis_hit` 命中） | `crisis_check` / 危机拦截检查 | output = 用户原文 | `hit`；回合 kind=`crisis`，随后成对落库、turn_persist、turn_id 进 outcome |
| 危机拦截未命中 | `crisis_check` / 危机拦截检查 | — | `miss`，继续 |
| `tools::maybe_answer` 命中 | `tool_intent` / 日历规则路由 | output = tool.reply | `hit`；kind=`tool`，tool_label=`calendar:{action}` |
| `maybe_answer` 未命中 | `tool_intent` / 日历规则路由 | — | `miss`，落入模型 |
| pass1（`dialogue.rs:212`） | `llm_round` / 第 1 轮 · 能力候选 | prompt = sys0 全文，output = raw1 | 解析失败 `direct`；解析出候选 `answered` |
| 候选全无效（`:238`） | `prose_retry` / 候选无效 · 追问直答 | output = prose_retry 返回值 | `ok` |
| pass2（`:287`） | `llm_round` / 第 2 轮 · 读详情决策 | prompt = sys1 全文，output = raw2 | prose `direct` / call `answered` / none `none` |
| level2 JSON 泄漏（`:297`） | `prose_retry` / JSON 泄漏 · 追问直答 | output = retry 值 | `ok` |
| 工具执行（`execute_audited`） | `tool_call` / 工具调用 · `{name}` | args_json、summary、duration | `ok` / `denied` / `error`（含 e 原文进 output） |
| pass3（`:393-397`） | `llm_round` / 第 3 轮 · 结果解读 | prompt = sys2 全文，output = raw3 全文（流式拼最终） | `answered` |
| level3 JSON 泄漏（`:408`） | `json_retry` / JSON 泄漏 · 重问直答 | output = retry / 兜底模板值 | `ok` |
| 纯模型路径（无 kit，`:594`） | `llm_round` / 直接对话 | prompt = system 全文，output = raw | `answered` |
| 过滤（每条模型路径末尾） | `filter` / 反谄媚过滤 | — | `pass`；filter_report 同步进主档 |

- 每个 LLM 轮用 `let t = Instant::now(); let raw = backend.reply(...); rec.llm_round(..., t.elapsed())` 计时；不记录 token 数（Ollama 后端未暴露，eval_count 属 llm 层——**本轮不做**，留后扩展）。
- **落库时机**：`chat_turn_inner` 每条 return 路径上，事件插入后**同一 lock scope**内 `trace_turn_persist`，返回前把 turn_id 填进 `DialogueOutcome`。

### 3.3 结构变更

- `DialogueOutcome` 增加 `pub turn_id: Option<i64>`（serde 序列化自动下发；前端 dialog 命令返回值即得，实时气泡直接挂）。
- `proactive_text`（`dialogue.rs:656`）：记 1 条 `llm_round`（title = 主动搭话 · 今日观察，prompt = system 全文、output = reply），kind=`proactive`，assistant_event_id 回填，user_event_id = NULL。
- `proactive:message` 事件载荷扩展为 `{ text, turn_id }`（多余字段对现有前端非破坏）。
- Mock backend 路径同样走 recorder（测试断言 trace 行存在即可）。

## 4. 命令层（S8-b）

| 命令 | 签名 | 行为 |
|---|---|---|
| `turn_trace_get` | `(store, turn_id: i64) -> Result<Option<TurnTraceRow + steps>, String>` | 详情页主查询 |
| `turn_trace_find_by_event` | `(store, event_id: i64) -> Result<Option<i64>, String>` | 气泡反查（详情入口兜底） |

- 两条命令注册进 `lib.rs` invoke_handler。
- **`dialogue_history` 不需要改**：`DialogueBubble.id` 已有（§0），前端接上即可。

## 5. 前端（S8-c / S8-d）

### 5.1 类型与封装（S8-b 前端侧）

- `src/lib/types.ts`：`TurnTrace`、`TraceStep`（字段与 Rust snake_case 对齐：`step_kind`、`args_json`…）。
- `src/lib/commands.ts`：`turnTraceGet(turnId)`、`turnTraceFindByEvent(eventId)`。

### 5.2 气泡操作条（S8-c）

- `chatStore.ChatMessage` 增加字段：`ts?: string`（ISO）、`eventId?: number`、`turnId?: number`。
- `ChatPane.bubblesFromHistory`：入参接 `id` → `eventId`，`ts` → 时间；bot 行的 `turnId` 渲染时懒查（首次 hover「查看详情」时 `turnTraceFindByEvent`，成功则缓存进该气泡 state，失败隐藏入口）——避免 prefill 时 30 次 IPC。
- 实时路径 `useChat.ts`：`outcome.turn_id` + 本地 `new Date().toISOString()` 写进 bot 气泡；user 气泡在 send 入口记 ts。
- 主动搭话：`useAppEvents` 接 `turn_id`。
- 新组件 `src/components/chat/MessageActions.tsx`：
  - 时间常驻灰字：今天 `HH:MM` / 昨天 `昨天 HH:MM` / 更早 `M-D HH:MM`（与现有 note 日期规则统一，抽 `format.ts` helper `msgTimeLabel(ts)`）。
  - `复制`：`navigator.clipboard.writeText(message.text)` + `toast("已复制")`；流式中不显示。
  - `查看详情 →`：仅当 `turnId` 已知或懒查命中时显示；`router.push(/trace/${turnId})`。
  - D2 交互：`.msg-actions` 默认 `opacity:0`，`.bubble:hover .msg-actions` / focus-within 时 `opacity:1`（globals.css 加一段，语义 class，无新 CSS 文件）；键盘可达（操作条内按钮可 Tab 聚焦触发 focus-within）。
- `MessageBubble.tsx` 接线：非 streaming 且 kind ∈ {user, bot} 时渲染 `<MessageActions />`；prediction / announcement / reminder 复用 `onStick` 既有动作，本轮不加操作条。
- AGENTS.md 约束：组件 <150 行；invoke 全走 `commands.ts`；无 inline style / 硬编码色。

### 5.3 追踪详情页（S8-d）— `src/app/trace/[turnId]/page.tsx`

结构（对应设计稿 2）：

1. `PageHeader` 复用：标题「回合 #{turnId} 的生成链路」+「← 返回对话」（`router.back()`）。
2. **摘要卡**：kind 徽标（模型回合 / 工具回合 / 危机拦截 / 主动搭话）+ 模型是否启用 + 总耗时 + 过滤结果；用户输入（secondary 色，取 `steps[0].output` 或主档 user 事件）与最终回复（primary 色）。
3. **时间线**：`trace_steps` 按 seq 渲染垂直时间线；拆子组件 `TraceStepCard.tsx`：
   - 节点色（CSS 变量语义 class）：灰=crisis_check/tool_intent，蓝=llm_round，绿=tool_call ok / filter pass，红=denied/error，橙=json_retry/prose_retry。
   - 行首 title，行尾 outcome + `duration_ms`。
   - **展开交互**：`llm_round` 卡展开显示 prompt 全文与 output（等宽 `pre-wrap`，各自带「复制」小按钮）；`tool_call` 卡展开 args_json（格式化）+ summary。默认折叠，点击切换。
4. **空态**：`turn_trace_get` 返回 None → 「该消息产生于链路追踪上线之前，没有留下记录」。
5. 反查路由支持 `/trace?event={eventId}`（`searchParams` 读到后先 `turnTraceFindByEvent` 再渲染同页），供历史气泡懒查命中后的跳转复用。
6. 数据获取失败静默转空态 + toast，不白屏。

### 5.4 测试（frontend/tests/，沿现有结构）

1. `MessageActions`：时间三种格式、复制触发 clipboard + toast、无 turnId 不渲染详情、streaming 不渲染（4）。
2. `/trace` 页：有数据渲染摘要 + 步骤数、空态文案、llm_round 展开含 prompt 文本（3）。

## 6. Rust 测试清单（dialogue.rs / db.rs tests，~10）

1. 迁移幂等：v8→v9、重复 migrate、旧数据无损（沿 spec-s2 惯例）。
2. crisis 路径：turn_trace kind=crisis + 1 条 crisis_check(hit) step + turn_id 回填 outcome（1）。
3. 工具命中路径：kind=tool、tool_label=calendar:*、tool_intent(hit) step（1）。
4. agent_pipeline 全链路（kit 带能力的 mock）：steps 顺序 = crisis_check(miss) → tool_intent(miss) → llm_round×3 → tool_call → filter；pass1/pass2/pass3 prompt 均非空、pass3 output = 最终回复（2：直答分支 + 调用分支）。
5. JSON 泄漏重试：level2/level3 各 1 条 prose_retry/json_retry step（1）。
6. 纯模型路径（无 kit）：kind=model、1 条 llm_round + 1 条 filter（1）。
7. `turn_trace_find_by_event`：user/assistant 两侧命中均返回同一 turn；陌生 id 返回 None（1）。
8. `trace_turn_persist` 事务性：steps 随 turn 一起可见；turn 不存在无孤儿 step（1）。
9. proactive：kind=proactive、user_event_id NULL、assistant_event_id 有值（1）。

## 7. 里程碑与验收

| 段 | 内容 | 门禁 |
|---|---|---|
| S8-a | schema v9 + db helper + TraceRecorder + 全路径埋点 + DialogueOutcome.turn_id | cargo test 全绿（新增 ~10） |
| S8-b | turn_trace_get / find_by_event 命令 + 前端 types/commands | cargo + tsc |
| S8-c | ChatMessage 字段 + MessageActions + useChat/useAppEvents/ChatPane 接线 + globals.css | vitest 全绿 + tsc |
| S8-d | /trace/[turnId] 页 + TraceStepCard + 空态 + ?event 反查 | vitest 全绿 + 真机验收 |

验收清单：

- [ ] cargo test 全绿；vitest 全绿；`npx tsc --noEmit` 干净
- [ ] 真机：发一条「明天有什么安排」→ 气泡 hover 出现时间/复制/详情 → 详情页时间线含 llm_round×3 + tool_call，prompt/返回值可展开
- [ ] 真机：crisis 词 → 详情页只有危机拦截一步；纯闲聊 → 单轮直答时间线
- [ ] 真机：重启后回放的历史气泡 hover → 懒查反查跳转详情页成功
- [ ] grep 确认：turn_trace/trace_steps 不出现在 ACTIVITY_KINDS 及任何派生事件写入路径

## 8. 边界（本轮不做）

- 不做 trace 检索 / 会话级聚合视图 / 导出。
- 不做精确 token 统计（llm 层未暴露，留后扩展）。
- 不截断、不清理 trace 数据（D1 全量永久保留；体积问题留给后续数据可携带 P2-5 一并裁决）。
- prediction / announcement / reminder 气泡不加操作条。

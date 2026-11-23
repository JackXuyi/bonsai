# plan-context — 上下文管理：token 预算 / 窗口压缩 / 多会话

| 文档信息 | 内容 |
|---|---|
| 版本 | v0.1（待裁决） |
| 日期 | 2026-11-23 |
| 状态 | 方案稿，未实施；「现状」全部基于 `98de10f` 之后的实况核查（带行号） |
| 上游输入 | `docs/zh/plan-dialogue-timeout.md`（2026-11-23 流式协议与 IPC 兜底）、`docs/zh/plan-planning.md`（多步任务把单轮拉长到分钟级）、`docs/zh/plan-agent-mcp-skill.md`（工具目录与会话维度） |
| 定位约束 | 本地 LLM 优先、本地零工具调用；记忆真值不删；能力不重复实现；助手非伴侣 |
| 一句话 | 上下文不是数据，是每轮按预算现算的**投影** |

---

## 0. 一句话结论

现在「上下文管理」只有一条规则：**取最近 6 轮**（`MAX_HISTORY_TURNS`），既不数 token，也没有「窗口」概念——所以它既不知道离溢出还有多远，也不知道这一轮花了多少钱。本方案补三件事，顺序是「先看得见，再压得住，最后分得开」：

1. **token 预算**：一个确定性估算器 + 一张窗口表 + 分段硬上限，让每轮输入在发出前就知道有多大；
2. **窗口压缩**：L0 确定性裁剪（丢最旧轮）→ L1 抽取式摘要（丢掉的轮压成短行落库）→ L2 夜间模型摘要（可选、默认关），外加轮内长结果与工具目录的有界化；
3. **多会话**：`sessions` 表 + 抽屉切换 + 每会话独立历史与摘要；记忆（facts / preferences / 画像）仍然全局——会话只是对话窗口。

> 关键认知：**压缩不是摘要功能，是预算的副产品**。先有预算，才知道该压谁、压多少、压完还差多少；反过来做「智能摘要」只会得到一个无法验收的黑盒。

### 关键取舍（先看这四条）

| 取舍 | 结论 | 理由 |
|---|---|---|
| 压缩会不会删数据 | **不删**。`events` 是真值，压缩只影响喂给模型的投影 | 用户说「忘掉」才删；否则任何压缩都必须是可逆的视图 |
| 摘要要不要调模型 | **热路径不调**。默认走抽取式（零请求、零延迟、零成本） | 本地 2B 摘要质量不稳；云端多一次请求要撞 `CLOUD_REQUEST_CAP=6` 与用户等待 |
| 要不要引 tokenizer | **不引**。字符分类估算，宁可高估 | tiktoken 要几 MB BPE 与网络；估算偏差可用 provider `usage` 回流校准 |
| 多会话加不加侧栏项 | **不加**。五条路由不动，入口在对话页抽屉 | 侧栏 92px 只放得下五项；会话是对话页的局部状态，不是一级页面 |

---

## 1. 现状锚点（全部实测，带行号）

| 现状 | 位置 | 对本方案的约束 |
|---|---|---|
| 历史只取**最近 6 轮**：SQL `LIMIT max_turns*2` 行，按 `session_id` + `kind='dialogue'` 过滤，配对成 Turn | `dialogue.rs:41`、`memory/db.rs:1845-1863` | 截断维度是「轮」，与内容长度无关：6 条 30 字闲聊和 6 段 3000 字回复被同等对待 |
| **全仓没有 token 计数**：无 estimator、无窗口常量、无用量记录 | `grep -rn "token"` 仅命中 `max_tokens` 与脱敏逻辑 | 预算与压缩都是净新增；也让「花了多少」目前完全不可观测 |
| 唯一的长度限制是**单条工具输出**：普通工具 summary ≤200 字，`memory_search` ≤1600 字，`fs_read` 默认 4000 / 上限 8000 字 | `agent_tools.rs:107`、`memory_search.rs:38`、`filetools.rs:40-42` | 单条有界、**总量无界**：8 次工具 × 8000 字 ≈ 6.4 万字一次进上下文 |
| 每轮 msgs = system + 6 轮历史 + 本轮用户 + 工具轮（assistant.tool_calls + tool 结果），**只增不减** | `dialogue.rs:741-747`、`925`、`1033` | 同一份历史在 ≤6 次请求里被重复发送、重复计费；轮内没有回收机制 |
| 系统提示词 = 环境段 + 画像（≤200 字，已有段级预算）+ L1 宪法 + L2 表达 + 关系段 + L3 状态 + 用户边界 + skill 段 | `dialogue.rs:303-345`、`persona/prompt.rs:99`（`PROFILE_BUDGET_CHARS=200`） | 「段级字符预算」是**既有惯例**，本方案只是把它推广到所有段，不是新发明 |
| 工具目录每轮全量下发：`tool_specs()` 把 `catalog_all()` 的 summary 直接当 description，**无长度上限** | `dialogue.rs:636-646` | MCP 工具描述来自外部服务器，长度不可控；多装几个 server 就会悄悄吃掉几千 token |
| 云端只有输出上限 `CLOUD_MAX_TOKENS=4096`，没有输入窗口概念；provider 返回的 `usage` 帧被解析器丢弃 | `llm/mod.rs:59`、`llm/cloud.rs:347` | 「窗口」必须由我们自己在客户端建模；`usage` 是现成的校准信号，目前白白扔掉 |
| 本地模型 `num_ctx` 显式缺省为 `0`（= 交给 Ollama 默认，通常 2048/4096） | `llm/ollama.rs:110,221` | 系统提示词约 1.2–1.5k tokens，本地默认窗口塞不下「system + 6 轮」；llama.cpp 系超窗时保留尾部、丢弃开头（**即最该保住的宪法**），且无任何报错 |
| 会话写死 `"default"`：三处硬编码 | `useChat.ts:19`、`ChatPane.tsx:113,150` | 后端会话维度早已齐备（`events.session_id`、`dialogue_history`、`forget_session`），缺的只是前端与列表 |
| 已有第二个特殊会话 `proactive`：主动搭话写在独立 session | `dialogue.rs:47,1893-1896`、`memory/db.rs:951` | 多会话必须回答「proactive 算谁的会话」，否则列表与回放会打架 |
| 遗忘不彻底：`forget_session` 只删 `events` + 其 `facts`；`forget_all` 也没清 `turn_trace` / `trace_steps` / `tasks` | `memory/db.rs:634-641`、`647-661` | 用户「忘掉这段对话」之后，`trace_steps.prompt` 里仍有 system + 原文全文。这是合规缺口，与上下文管理同源（都是「文本存在哪」） |
| `forgetSession()` 前端封装已存在但**零消费** | `commands.ts:170` | 多会话删除入口落地时正好消费掉它，别再留一个孤儿封装 |

---

## 2. 能力边界与取舍

### 2.1 三件事各自的去留

| 能力 | 做不做 | 本版做到什么程度 |
|---|---|---|
| token 估算 | **做** | 确定性纯函数，无外部依赖；只求「不低估」，不追求与 tokenizer 逐 token 相等 |
| 窗口表 | **做** | 内置 model → window 映射 + settings 覆盖；未知模型取保守值 |
| 分段预算 | **做** | 9 个段各自的硬上限（§4.2），超了按既定顺序让路 |
| L0 裁剪 | **做** | 丢最旧轮，最近 2 轮必留；被丢的轮必须进 digest，不允许静默消失 |
| L1 抽取式摘要 | **做** | 规则抽取 + 字数上限 + 落库（`sessions.digest`），零模型 |
| L2 模型摘要 | **做但不默认开** | 夜间/空闲跑本地模型重写 digest；开关在设置页，默认关 |
| 轮内长结果压缩 | **做** | 只替换 `tool` 消息正文，**不删** `assistant(tool_calls)`/`tool` 配对（协议硬约束） |
| 工具目录上限 | **做** | 条目数 + 每条描述长度的双重截断，trace 记 `tools_truncated` |
| 多会话 | **做** | 建/改名/归档/删/切换 + 独立历史 + 独立 digest；标题取首条用户消息 |
| 会话级统计 | **做** | trace 每轮记 `prompt_tokens_est` / `prompt_tokens_actual` / `dropped_turns` |

### 2.2 明确不做（本方案范围外）

- **向量检索 / 语义压缩**：`db.rs:358` 的 TODO 仍挂着，等 sqlite-vec 路线定了再说。上下文压缩不做 embedding 召回。
- **模型生成会话标题**：多一次请求、多一个失败点；先取用户首句截断。
- **会话树 / 分支（fork）/ 编辑历史消息**：产品语义是「陪伴」，不是 IDE。
- **跨设备同步会话**：本地优先，不做云同步。
- **对本地 2B 开启工具调用**：硬约束不变；压缩只发 `reply()`，不发 `tools[]`。
- **精确 tokenizer**：不引 tiktoken / HF tokenizers；偏差用 `usage` 校准。
- **按消息重排 / 按相关性挑历史**：会让对话时序错乱，且无法向用户解释「为什么它忘了上一句」。
- **proactive 会话并入用户会话**：见 D6；本版不动它的落库方式。

---

## 3. 不变量（违反即回滚）

| 编号 | 不变量 | 兑现方式 |
|---|---|---|
| I1 | **真值不删**：压缩只影响投影，`events` 不因压缩而少一行 | 压缩函数只读 `events`，不写、不删 |
| I2 | **宪法不可压**：system 段中的 L1 宪法、用户边界永不裁剪 | 裁剪只作用于「历史轮」与「工具结果」；system 段只允许 profile 按既有 200 字规则缩 |
| I3 | **单点实现**：所有裁剪/摘要逻辑只在 `context.rs` | `dialogue.rs` 只调用 `context::build()` / `context::compact()`，不得内联第二套逻辑 |
| I4 | **本地零工具不变**：压缩与摘要不产生任何 tool 调用 | 摘要走 `reply()`（无 `tools[]`），且默认不调模型 |
| I5 | **压缩可观察**：每次真的丢了东西都要落 trace | `dropped_turns` / `tool_chars_elided` / `tools_truncated` 三个计数 |
| I6 | **协议不破坏**：`assistant(tool_calls)` 与 `tool` 消息必须成对且相邻 | 轮内压缩只换 `tool` 的 content，不删消息 |
| I7 | **切换不并发**：一轮未终态时不允许切/建会话 | 前端禁用入口；需要切先 `dialogueCancel` |

---

## 4. 总体架构

### 4.1 三个数字

```
window(model)      硬窗口（来自窗口表，未知模型取保守值）
soft  = 32000      软预算：成本闸，可配（settings: ctx_soft_budget）
usable = min(window, soft) - 4096        # 4096 = CLOUD_MAX_TOKENS 答复预留
```

- **窗口**决定「会不会 400」；**软预算**决定「这一轮值多少钱」。两者必须分开，否则窗口大的模型会一路烧到顶。
- 本地模型 `usable` 直接由 `num_ctx` 决定（建议 8192，见 D5），不参与软预算。

### 4.2 分段预算（从上到下优先级递减）

| 段 | 上限（tokens） | 超了怎么办 |
|---|---|---|
| L1 宪法 + 关系 + 状态 | 1500 | **永不裁**；超了只记 trace 告警（说明人格卡写太胖） |
| 用户边界 | 300 | **永不裁**（安全相关） |
| 环境段 | 100 | 永不裁（一行时间/时区） |
| 画像 | 250 | 按既有 `PROFILE_BUDGET_CHARS=200` 字规则截到放得下为止 |
| skill 段 | 800 | 按既有 `SIMULTANEOUS_SKILLS` 限额，先到先得，超出的不注入 |
| 工具目录 | 1500 | 条目与单条描述双截断，trace 记 `tools_truncated` |
| digest（早期摘要） | 600 | L1 重写时按上限丢最旧条目 |
| 历史轮 | `usable` 余量，最多 12 轮 | 从最旧丢，被丢的进 digest |
| 本轮用户消息 | 4000 | **不截**（用户原话必须完整）；改为从历史让路 |
| 本轮工具结果 | `usable × 40%`，单条 1500 | 长结果降级 + 省略标记（§6.6） |

> 设计意图：**先保「它现在是谁」和「用户这一句」，最后才保「上周聊了什么」**。当前实现恰好相反——历史是固定 6 轮，system 反而可能被本地窗口挤掉。

### 4.3 三级压缩阶梯

| 级 | 时机 | 手段 | 成本 | 是否落库 |
|---|---|---|---|---|
| L0 | 每轮装配时 | 按预算丢最旧轮；最近 2 轮必留 | 零 | 否（派生） |
| L1 | L0 真的丢了轮 | 抽取式短行（诉求 → 结果），≤60 字/条，总 ≤600 字 | 零 | 是（`sessions.digest`） |
| L2 | 夜间巩固之后 | 本地模型把 digest 重写成 ≤300 字人话 | 一次本地推理 | 是（覆盖 digest，失败保持 L1） |
| 轮内 | 每次 `chat_with_tools` 前 | 只截超长 `tool` 结果 | 零 | 否 |

> L2 与 L1 是**覆盖关系**而非叠加：L2 成功就用 L2 的文本，失败就继续用 L1。这样 digest 永远只有一份，不会出现「摘要的摘要」。

---

## 5. 数据模型（schema v15）

### 5.1 新表 `sessions`

```sql
CREATE TABLE IF NOT EXISTS sessions (
    id            TEXT PRIMARY KEY,          -- 'default' | uuid
    title         TEXT NOT NULL,             -- 首条用户消息前 24 字（零模型）
    created_at    TEXT NOT NULL,
    updated_at    TEXT NOT NULL,             -- 每次落对话事件时 touch，列表按它倒序
    archived      INTEGER NOT NULL DEFAULT 0,
    digest        TEXT,                      -- L1/L2 摘要正文（NULL = 还没压过）
    digest_upto_event_id INTEGER,            -- 已合并进 digest 的最大 events.id（防重复合并）
    message_count INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_sessions_updated ON sessions(archived, updated_at DESC);
```

### 5.2 `events` 补索引（性能，不是装饰）

```sql
CREATE INDEX IF NOT EXISTS idx_events_session ON events(session_id, id);
```

现状按 `session_id + kind` 查最近 N 行只能走 `idx_events_ts` 再过滤（`db.rs:216-218` 没有会话索引）。多会话之后每个会话都要查，索引从「可有可无」变成「必须有」。

### 5.3 `turn_trace` 补三列（观测）

| 列 | 类型 | 含义 |
|---|---|---|
| `prompt_tokens_est` | INTEGER | 本轮最后一次请求的估算输入 tokens |
| `prompt_tokens_actual` | INTEGER | provider `usage.prompt_tokens`（有就填，没有留 NULL） |
| `dropped_turns` | INTEGER | 本轮被 L0 丢掉的历史轮数（0 = 没压） |

迁移沿用既有范式：`execute_batch(SCHEMA)` **之后**再 `add_column_if_missing`（v10 教训：先建表后加列，顺序反了会在老库上失败）。

### 5.4 迁移与回填

1. `SCHEMA_VERSION` 14 → **15**，同时 `set_user_version`（v10 教训：只改常量不 bump，老库不会迁移）。
2. 建 `sessions` 表 + 两个索引（`IF NOT EXISTS` 幂等）。
3. **回填 `default`**：若 `events` 里存在 `session_id='default'` 且 `sessions` 无此行 → 插入一行，`title` 取该会话首条 user 消息前 24 字（去掉换行），空则「默认会话」；`updated_at` 取该会话最后一条事件时间。
4. **`proactive` 不建行、不迁移、不进列表**（见 D6）。它继续按原样落 `events`，实时气泡照旧。
5. 回填**不写 digest**：第一次真正超预算时才生成，避免给所有老用户凭空造摘要。

### 5.5 遗忘完整性补丁（同批修，标 P0）

| 函数 | 现状 | 补成 |
|---|---|---|
| `forget_session` | 删 `events` + 其 `facts` | 同事务再删：该会话的 `turn_trace`、`trace_steps`（按 turn 级联）、`tasks`（`goal` 含原文）、`sessions` 行 |
| `forget_all` | 漏 `turn_trace` / `trace_steps` / `tasks` / `sessions` | 全部补上 |

理由：`trace_steps.prompt` 存的是**未截断的 system prompt**，`turn_trace.user_text/reply_text` 存的是原文。用户点「忘掉这段」之后这些东西还在库里，属于「说忘了其实没忘」，比上下文超预算严重。

---

## 6. 模块设计

### 6.1 估算器（`context.rs::estimate`）

```
tokens(s) = 4                                   # 每条消息结构开销
          + cjk_chars × 1.0                     # 汉字/全角标点：1 字 ≈ 1 token（保守高估）
          + ascii_alnum_chars / 4               # 英文/数字：4 字符 ≈ 1 token
          + other_chars × 0.75                  # 代码符号/emoji 等
```

- 方向要求：**宁可高估**。低估的代价是 400 或本地静默截断；高估的代价只是少放一轮历史。
- 工具消息额外 +8（`tool_call_id` 与角色包装）；`assistant(tool_calls)` 的 JSON 参数按序列化文本计。
- 单测锁四类：纯中文、纯英文、中英混排、代码块；断言「估计值 ≥ 字符数 ÷ 4」这类下界，并断言顺序稳定（同输入同输出，纯函数无状态）。
- 校准钩子：provider `usage.prompt_tokens` 与估算值同时落 trace，偏差持续 >20% 时再调系数（本版只收集，不自动调）。

### 6.2 窗口表（`context.rs::window_for`）

| provider / model 前缀 | window | 依据 |
|---|---|---|
| `deepseek`（deepseek-chat / reasoner） | 128_000 | 官方 API 文档：V3.2，128K 上下文 |
| `claude` | 200_000 | Anthropic 公开规格 |
| `openai` / `gpt-` | 128_000 | 保守取常用档 |
| 本地（`ollama` / `serve`） | `num_ctx`（建议 8192） | 由我们显式设定，不再留 0 |
| 未知 | 32_000 | 保守兜底 |

覆盖顺序：`settings.ctx_window`（手填）> 表 > 兜底。写进设置页「高级」区，标注「不确定就别动」。

### 6.3 装配（`context::build`）

```rust
// T2 实际落地形状（`truncated_tools` 随 T3 轮内压缩一起加，先不占位）
pub struct BuildInput<'a> {
    pub env: &'a str,              // 环境段（拼接在最前，与旧行为一致）
    pub body: &'a str,             // 画像 + 宪法 + 边界 + skill 段
    pub digest: Option<&'a str>,   // T6 才写库，现一律 None
    pub history: Vec<Turn>,        // 已按 12 轮上限读出，oldest first
    pub user_text: &'a str,        // 不截，只计估算
    pub reserve_tool_room: bool,   // 云端工具循环 true；本地模式 false
}
pub struct Built {
    pub system: String,            // env + body（+ digest 段）
    pub turns: Vec<Turn>,          // 已按预算裁剪，oldest first
    pub dropped: usize,            // 丢了几轮（给 trace）
    pub est_tokens: usize,
}
```

流程：

1. 取不可压段（环境 / 画像 / 宪法 / 边界 / skill），算 `fixed`；
2. 取 `sessions.digest`（无则空），算入 `fixed`；
3. 取历史（上限 12 轮），**从新到旧**累加，装不下就停；最近 2 轮即使装不下也留（并把余量从工具额度里借）；
4. 丢掉的轮交给 L1 抽成短行，合并进 digest（写库，见 §6.4）；
5. 本轮用户消息无条件保留；
6. 返回 `system + turns + user`，并回报 `est_tokens`。

> **`reserve_tool_room` 的由来（T2 实测修正）**：原方案一律按「可用 − 固定 − 工具额度」算历史空间。但本地模式**零工具调用**（2B 不挂 `tools[]`），预留 40% 等于白扔 1.6k——8192 窗口下历史会从 6 轮掉到 2 轮。因此装配层只在「云端 + 本轮确有工具能力」时预留；本地模式 `room = usable − fixed`。

> `digest` 进 system 段末尾（标 `# 较早对话摘要（系统压缩，非用户原话）`），不进历史数组——避免它被当成「用户说过的话」参与配对。

### 6.4 L1 抽取式摘要（`context::digest_merge`）

- 每条：`- [MM-DD] 用户诉求前 30 字 → 助手首句前 30 字`；单条 ≤60 字。
- 上限：总 ≤600 字；超了从最旧条目丢。
- 幂等键：`digest_upto_event_id`，只合并 id 更大的轮，重复调用不重复加行。
- **只记事实不记语气**：摘要是给模型当背景的，不负责复现人格；人格由 system 段保证。
- 抽取失败（消息为空/全符号）就跳过该轮，不占额度。

### 6.5 L2 模型摘要（默认关）

- 触发：夜间 `consolidate` 跑完之后，同一线程接着处理「昨天有 digest 更新的会话」，最多 3 个。
- 通道：本地 `reply()`（无 `tools[]`），system 用一句固定指令，不注入人格卡（省预算、防人格漂移）。
- 输出约束：≤300 字，纯文本；解析失败或超时 10s → 保留 L1 文本，不重试。
- 开关：`settings.ctx_digest_model = "off" | "local"`，默认 `off`。

### 6.6 轮内压缩（`context::compact`）

只做两件事，都在每次 `chat_with_tools` 之前调用：

1. **长 `tool` 结果降级**：单条超 1500 tokens → 保留 head 70% + tail 20% + `\n…（已省略 N 字）`。
2. **工具目录截断**：`tool_specs()` 后按「总 tokens ≤1500」裁剪——先截单条 description 到 240 字，再按条目顺序砍到预算内；被砍掉的工具名记 trace。

**绝不删 `assistant(tool_calls)` / `tool` 配对**：Chat Completion 协议要求 `tool_call_id` 一一对应，删一条就 400。所以轮内压缩只能是「换内容」，不能是「删消息」。

### 6.7 会话命令（新模块 `src-tauri/src/session.rs`）

| 命令 | 签名 | 行为 |
|---|---|---|
| `session_list` | `(limit?) -> Vec<SessionRow>` | 未归档，按 `updated_at DESC`，默认 50 |
| `session_create` | `(title?) -> SessionRow` | 生成 uuid；无标题则「新对话」 |
| `session_rename` | `(id, title)` | 截 60 字；空标题拒绝 |
| `session_archive` | `(id, archived)` | 归档/取消归档（数据不动） |
| `session_delete` | `(id)` | 拒绝删 `default`；调 `forget_session` 的加强版（§5.5）；二次确认在前端 |

- `default` 会话**不可删、不可归档**：它是老数据的锚点，删了等于让历史消失得无影无踪。
- `touch`：每次落 user/assistant 事件时更新 `updated_at` 与 `message_count`（同一事务，失败不阻塞回复）。
- 标题规则（零模型）：首条用户消息 → 去掉换行与多余空白 → 取前 24 字 → 末尾加「…」如果被截。

### 6.8 前端会话切换

- 新增 `lib/stores/sessionStore.ts`：`list` / `currentId` / `loading` / `switchTo(id)` / `create()` / `rename()` / `archive()` / `remove()`。
- `useChat` 的 `SESSION_ID` 常量删除，改读 store（`sessionStore.getState().currentId`）。
- `ChatPane` 的两处 `dialogueHistory("default", …)` 改传入 `currentId`；切换时：`chatStore.clear()` → 拉第一页 → 重置分页游标（`oldestIdRef` / `hasMoreRef`）。
- **在飞禁止切换**：`chatting === true` 时抽屉里的切换/新建置灰，提示「等它说完」；不静默 cancel（用户可能只是手滑）。

### 6.9 可观察性

- 每轮收尾写 trace：`prompt_tokens_est` / `prompt_tokens_actual` / `dropped_turns`。
- `/trace` 顶部加一行：`上下文 3.4k / 32k · 压缩 2 轮`（`dropped_turns>0` 时数字高亮）。
- 工具页**不加**新卡：面板已经够长，预算信息属于「出问题时才看」的明细，放 trace 即可。

---

## 7. 管线接线一览

| 文件 | 改动 | 约行数 |
|---|---|---|
| `src-tauri/src/context.rs`（新） | 估算器 + 窗口表 + `build` / `compact` / `digest_merge` + 单测 | ~450 |
| `src-tauri/src/dialogue.rs` | `741-747` 改用 `context::build`；`1531-1536` 传入 session 元数据；循环内每次 `chat_with_tools` 前 `context::compact`；trace 收尾写三个计数 | ~60 |
| `src-tauri/src/llm/ollama.rs` | `num_ctx` 常量从 0 改为显式值（D5） | ~10 |
| `src-tauri/src/llm/cloud.rs` | SSE 末帧 `usage` 提取（`prompt_tokens`），失败不影响正文 | ~25 |
| `src-tauri/src/memory/db.rs` | v15 迁移、`sessions` CRUD、`forget_*` 补丁、`turn_trace` 三列 | ~180 |
| `src-tauri/src/session.rs`（新） | 5 个 Tauri 命令 + 标题规则 | ~150 |
| `src-tauri/src/lib.rs` | 注册 5 个命令 | ~6 |
| `frontend/src/lib/stores/sessionStore.ts`（新） | 列表/当前/切换/CRUD | ~90 |
| `frontend/src/lib/commands.ts` / `types.ts` | 5 个封装 + `SessionRow` / `BuiltStats` | ~40 |
| `frontend/src/hooks/useChat.ts` | 去掉 `SESSION_ID`；切换后重放 | ~20 |
| `frontend/src/components/chat/ChatPane.tsx` / `SessionDrawer.tsx`（新） | 抽屉 + 切换 + 禁止并发 | ~160 |
| `frontend/src/app/styles/chat.css` | 抽屉与列表样式（复用既有变量） | ~80 |
| `frontend/src/components/trace/TraceTimeline.tsx` | 顶部上下文行 | ~30 |

---

## 8. UI 呈现（D7 裁决：抽屉，不加侧栏项）

**入口**：对话页右上角一个「会话」按钮（图标 + 当前标题截断），点击从右侧滑出抽屉（宽 260px，复用 `.card` 的底色与 `--line` 分割）。

**列表行**：标题（单行省略）· 相对时间（今天/昨天/MM-DD）· 条数；当前行左侧一条 2px 竖线（`--accent`）+ 稍深底色。

**操作**：
- 顶部「新对话」按钮（全宽，细边）；
- 悬浮/聚焦行尾出现「…」→ 重命名（行内编辑）/ 归档 / 删除（删除红字，二次确认弹层写清「这会连带清掉这段对话的生成记录」）；
- 底部「已归档（N）」折叠区，可恢复。

**不做的**：搜索框、标签、置顶、拖拽排序——会话数量在个人使用里很难超过几十条，加了就是负担。

---

## 9. 分期里程碑（逐可编译单元提交）

| 期 | 目标 | 主要文件 | 门禁 | 提交 subject |
|---|---|---|---|---|
| T1 | 估算器 + 窗口表 + 分段预算常量（**零接线**） | `context.rs`（只含纯函数） | `cargo test --lib` 新增 ≥8 用例 | `feat(context): token 估算与预算表` |
| T2 | `context::build` 装配 + L0 裁剪，接两路 | `context.rs`、`dialogue.rs` | 预算充足时与现状**逐条等价**（单测断言语序与内容） | `feat(context): 按预算装配上下文` |
| T3 | 轮内压缩 + 工具目录上限 | `context.rs`、`dialogue.rs` | 长结果降级、配对不被破坏（断言 `tool_call_id` 对齐） | `feat(context): 轮内结果与目录有界化` |
| T4 | schema v15 + 会话 CRUD + 遗忘补丁 | `memory/db.rs`、`session.rs`、`lib.rs` | 迁移测试（v14 老库 → v15 有 default 行、proactive 无行） | `feat(context): schema v15 会话表与遗忘补全` |
| T5 | 前端多会话 | `sessionStore.ts`、`SessionDrawer.tsx`、`ChatPane.tsx`、`useChat.ts` | `tsc` 0 错；金丝雀：切换后不串味 | `feat(context): 对话页会话抽屉` |
| T6 | L1 digest 落库 + L2 夜间摘要（默认关） | `context.rs`、`memory/consolidate.rs` | 幂等（重复压缩不重复加行） | `feat(context): 早期对话摘要` |
| T7 | 观测 + 真机验收 | `TraceTimeline.tsx`、`llm/cloud.rs` | 全量门禁 + 真机清单 | `feat(context): 上下文用量可观测` |

依赖：T2 依赖 T1；T6 依赖 T4（要 `sessions.digest` 列）；T5 依赖 T4；T7 依赖 T2/T4。

---

## 10. 验收标准

| 编号 | Given | When | Then |
|---|---|---|---|
| A1 | 纯中文 / 纯英文 / 中英混排 / 代码块 | 调 `estimate` | 结果稳定、可复现、且高于「字符数 ÷ 4」下界 |
| A2 | 预算充足（6 轮都很短） | `context::build` | 输出的 system + 6 轮 + 用户消息，与现状逐条一致（不因新代码改变行为） |
| A3 | 历史远超预算 | `build` | 从最旧开始丢；最近 2 轮必在；`dropped > 0` 且被计数 |
| A4 | 用户边界很多 / 画像很长 | `build` | 宪法与边界一字不丢；画像按 200 字规则截断 |
| A5 | 一次真丢了 3 轮 | `build` | digest 多出 3 行，总长 ≤600 字，system 里出现「较早对话摘要」标记 |
| A6 | 同一会话连续两轮都触发压缩 | 第二轮 `build` | `digest_upto_event_id` 前进，不重复合并同一轮 |
| A7 | 装 5 个 MCP server（描述都很长） | 一轮对话 | 目录被裁到 1500 tokens 内，trace 记 `tools_truncated` |
| A8 | `fs_read` 返回 8000 字 | 本轮超预算 | 该 `tool` 消息被降级并带省略标记；`tool_call_id` 仍与 assistant 配对 |
| A9 | 本地模式 | 任意一轮 | 请求带显式 `num_ctx`；输入被压进该预算内（不再依赖 Ollama 默认） |
| A10 | 任意一轮结束 | 看 `/trace` | 有 `上下文 X / Y · 压缩 Z 轮`，数字与 trace 三列一致 |
| A11 | 新建会话 | 立即发一条 | 历史为空，**不出现**默认会话的上下文 |
| A12 | 有两个会话各 30 条 | 切换 | 消息流立刻变成目标会话内容，旧气泡不残留 |
| A13 | 一轮在飞 | 点抽屉里的切换 | 入口置灰/拒绝，提示「等它说完」 |
| A14 | 删除一个会话 | 确认 | `events`/`facts`/`turn_trace`/`trace_steps`/`tasks`/`sessions` 都查不到该会话；列表同步 |
| A15 | 归档 | 看列表 | 不在主列表；恢复后原样出现，消息完整 |
| A16 | 点「忘记一切」 | 查 `trace_steps.prompt` | 一行都查不到（含 system 全文） |
| A17 | provider 返回 `usage` | 一轮结束 | `prompt_tokens_actual` 有值；没返回时留 NULL 不报错 |

---

## 11. 风险矩阵

| 风险 | 影响 | 概率 | 缓解 |
|---|---|---|---|
| 估算偏差大（尤其中英混排） | 预算浪费或仍溢出 | 中 | 保守系数（汉字 1.0）+ 软预算留 25% 余量 + `usage` 校准 |
| 压缩导致「它忘了我刚说的」 | 体验崩塌，用户不信任 | 中 | 最近 2 轮必留；digest 明确标注「系统压缩」；A3/A5 锁死 |
| 摘要累积成「摘要的摘要」 | 信息逐轮衰减 | 中 | L1 只压「原始轮」，digest 不参与再压缩；L2 覆盖不叠加 |
| 本地 8192 窗口让 2B 变慢/吃内存 | 首字延迟上升 | 中 | 真机实测；可回落到 4096（`num_ctx` 走 settings） |
| 多会话让 proactive 语义混乱 | 主动搭话说给了空气 | 中 | D6：proactive 保持独立流，不进任何会话的历史 |
| schema v15 迁移在老库失败 | 用户升级后打不开 | 低 | 迁移测试（v14 样本库）+ `IF NOT EXISTS` + 先建表后加列 |
| 前端切换与会话在飞竞争 | 消息串味 | 低 | I7 + A13；后端按 session 落库，重复事件用 id 去重 |
| 会话变多导致列表变慢 | 打开抽屉卡 | 低 | 索引 + limit 50 + 归档 |

---

## 12. 待裁决决策点

| 编号 | 决策 | 选项 | 建议 | 影响 |
|---|---|---|---|---|
| **D1** | 软预算上界 | 24k / **32k** / 不设 | **32k** | 直接决定每轮最多花多少输入 token；32k 对陪伴对话足够，且离 128k 窗口有余量 |
| **D2** | 热路径能不能多一次模型摘要 | **不能（确定性）** / 能（超预算时先摘要再答） | **不能** | 能 = 每轮最坏 +1 请求 + 数秒延迟，且撞 `CLOUD_REQUEST_CAP`；代价是摘要质量低（抽取式） |
| **D3** | digest 形态 | **结构化短行** / 一段人话 | **短行** | 短行可数、可截、可去重；人话需要模型，且长度不可控 |
| **D4** | L2 夜间模型摘要 | **默认关** / 默认开（local） | **默认关** | 默认开会让夜间任务变长，且 2B 摘要质量不稳；做成开关让用户自己选 |
| **D5** | 本地 `num_ctx` | **8192** / 4096 / 保持默认 0 | **8192** | 保持 0 意味着宪法可能被静默截掉；8192 需真机验延迟 |
| **D6** | proactive 算谁的会话 | **不进列表、不迁移** / 并入当前会话 | **不进列表** | 并入会让「它主动说的话」混进用户会话历史，也让 digest 语义变脏 |
| **D7** | 会话入口 | **对话页抽屉** / 侧栏第六项 | **抽屉** | 侧栏放不下第六项（92px），且会话不是一级页面 |
| **D8** | 遗忘补丁是否随本方案做 | **做（P0）** / 单独立项 | **做** | 只多 ~30 行，但能补掉「说忘了还在库里」的合规缺口 |

> D1–D3 是主干，定了才能开工；D4–D8 可以边做边定，但 D5 建议在做 T2 前定（它决定本地预算数字）。
>
> **实施状态（2026-11-23）**：D1 按建议值 `32000` 落地（`SOFT_BUDGET_DEFAULT`，可被 `Budget::new` 覆盖）；**D5 裁决 8192（T2 已落地，`llm/ollama.rs::NUM_CTX`），历史轮上限同步 6 → 12（仍由预算兜底）**。D2 及 D6–D8 仍在对应期开工前确认。

---

## 13. 实施记录

| 期 | 提交 | 门禁 | 备注 |
|---|---|---|---|
| T1 | `b2737c3` | `cargo test --lib` 486 passed（+16：估算器 6 / 窗口表 3 / 预算 5 / 分段表 2）；`cargo check` 仅既有 `dialogue.rs:101` dead_code | `context.rs` 落地：估算器（汉字 1.0 / 拉丁 1/4 / 符号 0.75，整体向上取整）+ 窗口表（deepseek 128k / claude 200k / openai 128k / 本地取 `num_ctx` / 未知 32k）+ `Budget`（`usable = min(window,soft) − 4096`、`tool_room` 40%、`history_room` 饱和减法）+ 7 个固定段上限。**零接线**：`dialogue.rs` 一行未改 |
| T2 | `bb4c8da` | `cargo test --lib` 495 passed（+9：装配 7 / 接线 1 / num_ctx 1）；`tsc --noEmit` 0 错；`vitest run` 170 passed；`cargo check` 仅既有 `TraceRecorder::new` dead_code | `context::build`（`BuildInput`/`Built`/L0/摘要位）+ 唯一装配点接线（原 `1531-1536` 的 `format!(env+body)` 改为 build）；**D5 = 8192** 落地（`ollama::NUM_CTX`，请求带显式 `num_ctx`）；历史上限 6 → `HISTORY_MAX_TURNS=12`；本地不预留给工具额度（云端才留）；trace 增 `context_budget` 步（est/usable/window/dropped） |
| T3 | | | |
| T4 | | | |
| T5 | | | |
| T6 | | | |
| T7 | | | |

---

## 14. 变更记录

| 版本 | 日期 | 说明 |
|---|---|---|
| v0.1 | 2026-11-23 | 初稿：现状锚点、三段预算、三级压缩、schema v15、T1–T7、A1–A17、D1–D8 |
| v0.1.1 | 2026-11-23 | T1 落地：`context.rs` 估算器 / 窗口表 / `Budget` / 分段上限；方案本身无变更 |
| v0.2 | 2026-11-23 | T2 落地：§6.3 换成实际签名（`BuildInput` / `reserve_tool_room`，`truncated_tools` 延到 T3）；D5 裁决 8192、历史 6 → 12；trace 增 `context_budget` 步 |

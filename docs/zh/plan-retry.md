# plan-retry — 通用重试与失败恢复

| 文档信息 | 内容 |
|---|---|
| 版本 | v0.1（待裁决） |
| 日期 | 2026-11-23 |
| 状态 | 方案稿，**未实施**；「现状」全部基于 `2ba2802` 的实况核查（带行号） |
| 上游输入 | `plan-dialogue-timeout.md`（流式协议与前端看门狗）、`plan-context.md`（摘要与 L2 的 CAS 写回）、`plan-planning.md`（多步任务把单轮拉长到分钟级）、`plan-task-card.md`（三档权限与审计） |
| 定位约束 | 本地 LLM 优先；失败不伪装成功；不静默重试有副作用的动作；能力不重复实现 |
| 一句话 | 失败不是异常分支，是**每层都有名字、有预算、有出口**的正常状态 |

---

## 0. 一句话结论

现在全仓有 **5 套各自为政**的失败处理，彼此不知道对方存在，而且几乎全部**不可观测**：

- 工具：Read 档固定 500ms、最多 2 次尝试（`dialogue.rs:1036-1056`）；
- sidecar：启动 3 次 × 60s 健康检查（`llm/serve.rs:163-185`）；
- web 工具：一次都不重试（`dialogue.rs:616-618`）；
- 夜间作业：失败后**每 60 秒**重跑一次，无退避、无上限、不写审计（`scheduler.rs:94-101`）；
- 模型：任何错误都被压成空串，直接换兜底句（`persona/eval.rs:171,189` → `dialogue.rs:1736`）。

根因只有一条：**`LlmError` 的三个变体从未到达任何决策点**。`DialogueBackend::reply*` 的返回类型是 `String`，`unwrap_or_default()` 把 401、429、502、超时、解码失败统一抹成 `""`，于是「该不该重试、该不该换条路、该跟用户说什么」全部无从判断——只剩一句「脑子转不动」。

本方案补三件事，顺序是「先有名字，再谈重试，最后才是恢复」：

1. **一个 `Failure`**（`kind` + `retryable` + `hint` + `detail`）：字符串前缀匹配退役，错误从传输层一路带到 UI；
2. **一个 `Backoff` + 一张分层策略表**：模型 / 流式 / 工具 / 轮 / 作业五层各自有次数、退避、上限与负责人；
3. **恢复语义**：部分成果保留、失败轮不污染历史、崩溃后不留 `running` 幽灵、sidecar 与 DB 各自有第二条命。

> 关键认知：**重试是「同一个动作再试一次」，恢复是「换个方式仍然交付」。** 现在两者混在一起（失败即兜底句），所以既没重试对，也没恢复住——做了 3 步工具的一轮，最后一次请求 502，用户得到的和「模型没启动」完全一样。

### 关键取舍（先看这四条）

| 取舍 | 结论 | 理由 |
|---|---|---|
| 按什么决定重试 | **按 `kind`**，不按调用点 | 现在 `should_retry` 用 `err.starts_with("bad_args")` 判定（`dialogue.rs:614-627`）；新增错误码 = 改一个函数 + 赌它拼写对 |
| 重试要不要让用户看见 | **要**：气泡里一行 + 「重试」按钮 | 现在只有 1.9 秒的 toast（`uiStore.ts:18`），用户低头回来就只剩一个句号 |
| 失败轮要不要落进历史 | **不要兜底回复**（只落 user 事件） | 否则下一轮 `dialogue_history` 与 digest 里就有一条「脑子转不动」的假回复，模型会当真的 |
| 引不引 retry/backoff crate | **不引**，30 行纯函数 | 项目零 async 重试框架；`Cargo.toml` 无 `rand`/`backoff`，抖动用确定性哈希即可，测试还能锁死 |

---

## 1. 现状锚点（全部实测，带行号）

| 现状 | 位置 | 对本方案的约束 |
|---|---|---|
| `LlmError` 三变体（`Transport` / `Status(u16)` / `Decode`）在 `DialogueBackend` 边界被 `unwrap_or_default()` 抹掉 | `llm/mod.rs:206-213`、`persona/eval.rs:167-190` | 分类必须发生在**抹掉之前**；`reply*` 的签名改不动（8 个 impl + 大量测试），走旁路 |
| `Status(u16)` 连响应体都不留：401 与 502 在类型上完全一样 | `llm/cloud.rs:72-74`、`llm/ollama.rs:103-104` | 需要把 body 前 200 字带进 `Failure.detail`，否则「key 过期」和「服务端崩了」永远同一句提示 |
| 云端循环**任何一次请求失败 → 整轮放弃**：`return Ok((String::new(), label, None, false))` | `dialogue.rs:951-969`（960 行） | 已执行的工具结果、任务卡（`None`）全部丢弃；用户重来一遍＝副作用再跑一次。这是本方案最大的恢复收益点 |
| 工具重试：`should_retry` 按字符串前缀判定；Read 档 + 非 web；固定 `RETRY_BACKOFF=500ms`；`attempts < 2` = 共 2 次尝试；无抖动 | `dialogue.rs:21`、`614-627`、`1036-1056` | 判定逻辑换成 `Failure`，退避换成 `Backoff`；Write 档「不重试」的结论**保留**（无幂等键） |
| `web_search` / `web_fetch` 一次都不重试（P2 裁决：搜索慢，重试会把单轮拖到分钟级） | `dialogue.rs:616-618` | 「DNS 抖动」与「结果不好」被同等对待；本方案只允许**传输层失败**重试 1 次，语义失败仍不重试 |
| sidecar：启动 3 次、每次最多 60s 轮询、`Outcome::{Ready,Retry,Fatal}` 三态 | `llm/serve.rs:149-185`、`187-283` | 启动路径已够好；缺的是**运行期**：进程崩了没人发现 |
| 夜间巩固失败只 `log::warn`；`decide_trigger` 因 `last_run_date` 仍非今天 → **每 60 秒重跑一次** | `scheduler.rs:94-101`、`103-160` | 无退避、无当日上限、无审计行；日志会被刷成噪音 |
| 审计表只写 `status='done'`：失败连一行都不留 | `consolidate.rs:317-337` | 「昨晚为什么没整理」永久不可查 |
| 7 个后台循环同一形状：`loop { sleep(60s); let _ = tick(); }`，**无 `catch_unwind`** | `scheduler.rs:94`、`relationship.rs:415-430`、`morph.rs:188-200`、`predict.rs:347-357`、`perception.rs:339-353`、`face.rs:110-128`（5s）、`dialogue.rs:2077-2101` | 一次 panic = 该线程永久停摆，且没有任何地方会告诉你它停了 |
| DB 打不开 → `setup` 返回 `Err` → 应用起不来 | `lib.rs:97`、`memory/db.rs:68-76` | 用户看到的是「闪退」，没有第二出口，也没有备份提示 |
| `MemoryStore(Mutex<Connection>)`：中毒后 `lock_conn` 把 poison 转成 sqlite 错误，**永久失败** | `dialogue.rs:286-292`、`memory/mod.rs:25-31` | 没有自愈路径，也没有「请重启」的明确提示；用户只会看到一串 sqlite 术语 |
| MCP 只在**启动时**重连一次（`reconnect_enabled_servers`） | `mcp_admin.rs:327-395`、`lib.rs:164-180` | 运行期断连（server 崩、网络断）→ `mcp_unreachable` 直接喂回模型，不尝试重连 |
| 前端终态事件：`turn_done` / `turn_error`（裸字符串） | `dialogue.rs:2225-2226`、`useChat.ts:24-29` | 前端拿不到 `kind`，只能整句展示；也无法判断该不该给「重试」 |
| 失败时只在气泡末尾追加一句话，**没有重试入口** | `useChat.ts:88-97` | `fail()` 把原因拼进正文，用户唯一的动作是重新打字 |
| 已流出的字在失败时**被覆盖**：`turn_done` 分支 `setText(bubbleId, outcome.reply)`，而失败时后端 `reply` = 兜底句 | `useChat.ts:145-165`（156 行）、`dialogue.rs:1736` | 用户眼看着打完的 200 字变成「脑子转不动」。这是纯 UX 事故，改起来只要一个取长规则 |
| 兜底句作为 assistant 事件落库 → 进入下一轮历史与 digest | `dialogue.rs:1736` 之后 | 假回复污染真值 |
| 错误文案靠字符串 `includes` 匹配，25 条 if | `commands.ts:292-338` | 新增错误码 = 前端改一处 + 赌命中；这是「分类法」缺失的另一半 |
| 模型下载失败无重试（`download_busy` / `pull` 子进程失败） | `llm_pull.rs:118` | 要用户再点一次；本方案**不动**（下载是显式动作，重试交给用户更有控制感） |

---

## 2. 能力边界

### 2.1 做

| 能力 | 做到什么程度 |
|---|---|
| 错误分类 | 11 个 `FailureKind`，从 LLM / 工具 / DB / MCP 四处汇聚到一处；每种给一句中文 hint |
| 退避 | 纯函数 `Backoff::delay(attempt)`：指数 + 上限 + 确定性抖动；每层一张参数表 |
| 分层重试 | 模型（幂等）、流式（仅未产出 delta 时）、工具（Read）、轮（重发请求不重跑工具）、作业（退避 + 当日上限） |
| 部分成果保留 | 云端循环失败但已执行过工具 → 用已有结果散文收尾；任务卡不丢 |
| 真值保护 | 失败轮不写兜底回复；`events` 里只留用户那句话 |
| 崩溃/资源恢复 | 启动清理 `running` 幽灵；sidecar 运行期看门狗；DB 打不开的第二个出口；Mutex 中毒的明确出口 |
| 观测 | `turn_trace.error_kind` + `attempts`、`consolidate_runs.error`、trace `retry` 步带 `kind/delay_ms` |

### 2.2 明确不做（本方案范围外）

| 不做 | 原因 |
|---|---|
| 失败后**自动切换**云端 ↔ 本地模型 | 静默换脑子比报错更吓人：人格表达会突变，用户不知道刚才那句话是谁说的。切换必须由用户在设置里做（D4） |
| Write 工具（副作用）自动重试 | 没有幂等键，重试可能写两遍文件 / 发两次请求。宁可在结果里写明「可能已执行」（§5.4） |
| 跨进程任务队列 / 断点续传 | 属于 P5「触发条件驱动」的范畴；本方案只做到「进程内不丢已完成的步」 |
| 改 `CLOUD_REQUEST_CAP` / `CLOUD_TOOL_CALL_CAP` | 那是预算问题（`plan-context.md` §4），不是重试问题；重试**不得**突破这两个数 |
| 全链路 `tracing` / 指标导出 | 现在有 `simplelog` + trace 两处足够；引入 metrics 是另一个方案 |
| 下载重试 / 安装包校验 | 显式动作，用户再点一次成本极低（`llm_pull.rs`） |

---

## 3. 不变量（违反即回滚）

1. **副作用不自动重试**：只有 `ToolAccess::Read` 与模型请求（纯读）可重试；Write / 发消息 / 写文件一律 0 次。
2. **已流出的字只增不减**：任何失败路径都不得把用户已经看到的正文变短（当前 `setText` 覆盖是反例）。
3. **失败必留痕**：每次失败至少留下一条可查记录（trace step / 审计行 / `log::warn` 三者之一），不能只有 toast。
4. **失败不污染真值**：兜底句不进 `events(kind='dialogue')`；`dialogue_history`、digest、回放看到的都是真发生过的对话。
5. **重试有界**：每层都有 `attempts` 上限 + 单轮总时间上限；不存在「一直重试」的循环。
6. **失败不伪装成功**：不得因为重试成功就把中途失败从 trace 里擦掉（`retry` 步必须留下第一次的 `kind`）。

---

## 4. 总体架构

### 4.1 `Failure`：唯一的失败形状

```rust
pub struct Failure {
    pub kind: FailureKind,
    pub retryable: bool,   // 由 kind 推出，允许调用点降级（如流式已产出）
    pub hint: &'static str,// 给用户的一句话（中文）
    pub detail: String,    // 给日志/trace：原始错误 + 响应体前 200 字（脱敏后）
}
```

| `FailureKind` | 来源 | 可重试 | 用户看到的话（hint） |
|---|---|---|---|
| `Transport` | 连接被拒 / DNS / 断流 | ✅ | 网络没连上，等一下再试 |
| `Timeout` | 请求超时 | ✅ | 它想太久了，没等到结果 |
| `RateLimit` | 429 | ✅（1 次） | 云端限流了，歇一会儿 |
| `Server` | 5xx | ✅（1 次） | 云端出故障了（不是你的问题） |
| `Auth` | 401 / 403 | ❌ | API Key 或额度不对，去设置里看一下 |
| `BadRequest` | 其它 4xx | ❌ | 请求被拒了（配置或上下文超限） |
| `Decode` | 协议 / JSON 解析失败 | ❌ | 回复格式不对，已丢弃 |
| `Interrupted` | 流式中途断（已产出部分） | ❌ | 说到一半断了 |
| `Denied` | 审批拒绝 / 权限不足 | ❌ | 这次调用被拒绝了 |
| `Unavailable` | MCP 未连接 / 服务不存在 | ❌（重连另算） | 这个工具暂时用不了 |
| `Internal` | DB / Mutex 中毒 / panic | ❌ | 它内部出错了，重启一下试试 |

> 分类函数三个入口：`Failure::from_llm(LlmError, body_snippet)`、`Failure::from_tool(&str)`（吃掉现有 `mcp_timeout` / `denied` / `bad_args` 等字符串，成为**唯一**的字符串→语义翻译点）、`Failure::from_db(rusqlite::Error)`。前端 `humanizeAgentError` 的 25 条 if 里，凡是「传输/限流/鉴权」类的一律退役，改由 `kind` 决定文案。

### 4.2 `Backoff`：纯函数，可测

```
delay(attempt) = min(base << (attempt-1), cap) ± jitter
jitter = hash(attempt, seed) % (delay/4)        // 确定性，测试可锁
```

| 层 | base | cap | 抖动 | 上限 |
|---|---|---|---|---|
| 模型请求（非流式） | 500ms | 4s | ±25% | 2 次重试（共 3 次请求） |
| 流式请求 | 500ms | 1s | ±25% | 1 次重试，且**仅在零 delta** 时 |
| 工具（Read） | 500ms | 2s | ±25% | 1 次重试（共 2 次尝试，与现状一致） |
| 夜间作业 | 60s | 30min | 无 | 当日 5 次，之后等明天 |
| sidecar 重启 | 3s | 60s | 无 | 每小时 1 次（进程确已退出才算） |

> 抖动不为「错开惊群」（桌面单用户没有惊群），只为**避免固定节奏撞上同一秒的限流窗口**，同时让测试能断言确切值。

### 4.3 策略表（谁负责哪一层）

| 层 | 触发点 | 重试什么 | 不重试什么 | 失败后交付什么 |
|---|---|---|---|---|
| 模型 | `LocalBackend::reply*` | 整次请求（幂等） | 已有 delta、`Auth`、`BadRequest` | `Failure` + 已流出文本 |
| 流式 | 同上 | 仅零 delta | 有任何 delta | `Interrupted` + 保留正文 |
| 工具 | `execute_audited` 外层 | Read 档 | Write / `Denied` / `BadArgs` | 失败事实回传模型（脱敏） |
| 轮 | `cloud_tool_loop` | 重发**同一条** msgs（工具结果已在序列里） | 重跑工具 | 有工具结果 → 散文收尾 / 本地拼装进度；零结果 → 兜底句 + `failure` |
| 作业 | `scheduler::tick` | 退避后重跑 | 当日超 5 次 | 审计行 `status='error'` |

---

## 5. 分层设计

### 5.1 模型层：不抹掉错误

`DialogueBackend` 有 8 个实现（`persona/eval.rs:109,158` + 6 个测试桩），改签名代价大且与失败处理无关。**取旁路**：

```rust
pub trait DialogueBackend {
    // ... 现有方法不变
    /// 上一轮 reply* 的失败（成功则 None）。默认 None —— 8 个 impl 零改动。
    fn take_failure(&mut self) -> Option<Failure> { None }
}
```

- `LocalBackend` 在 `reply` / `reply_streaming` 里把 `Err(e)` 存进 `last_failure`，`take_failure()` 取走（取走即清空，避免串到下一轮）。
- `reply_streaming` 额外记 `got_delta: bool`：**这是「能不能重试」的唯一判据**（有字已发出去就不能重发，否则用户会看到重复的半句话）。
- 重试发生在 `LocalBackend` 内部（它持有 client），循环两次后仍失败则返回已积累的文本 + `take_failure()` 交给上层。

`chat_turn_inner` 的改动：

```
raw = backend.reply_streaming(...)
failure = backend.take_failure()
if failure.is_some() && raw.trim().is_empty() -> 本轮 = 失败轮（§5.2 的落库规则）
if failure.is_some() && !raw.trim().is_empty() -> 本轮 = 部分成功：正文照发，尾部附「——说到一半断了」
```

`Failure.detail` 进 trace 的 `llm_round` output（现在那里是空串：`dialogue.rs:953-959` 的 `rec.llm_round(..., "", "error", ...)` 只有结论没有原因）。

### 5.2 轮层：不丢已完成的步

云端循环的失败分支（`dialogue.rs:951-969`）从「整轮放弃」改为三段式：

```rust
Err(e) => {
    let f = Failure::from_llm(e, body_snippet);
    rec.llm_round(..., "error");            // detail 带 kind
    if executed > 0 {
        // ① 已经跑过工具：允许一次「重发同一 msgs」（工具结果都在 msgs 里，不重跑）
        // ② 仍失败：用已有 summaries 让模型做一次「基于已有结果收尾」的请求
        // ③ 连这次也失败：本地拼装「做到了 X / Y，剩下没做完」，而不是兜底句
    }
    return ...; // 带 task card + failure
}
```

三条边界：

- **不重跑工具**：`msgs` 里 `assistant(tool_calls)` 与 `tool` 结果已配对，重发请求是纯读，符合不变量 1。
- **收尾请求不携带 `tools`**（`force_answer` 同样的手法，`dialogue.rs:944`），模型只能写散文。
- **本地拼装兜底**：连模型都叫不动时，用 `summaries` 拼「我做到了：①…②…，剩下的下次接着来」，并附 `failure.hint`。这比「脑子转不动」信息量高一个数量级。

### 5.3 失败轮的落库（D1）

现状：兜底句作为 assistant 事件落库 → 污染历史。

改为：

| 场景 | user 事件 | assistant 事件 | `turn_trace` | 回放表现 |
|---|---|---|---|---|
| 完全失败（无正文） | ✅ 落 | ❌ 不落 | kind='model', `error_kind` 有值 | 一条没有回复的 user 气泡（诚实） |
| 部分成功（有正文） | ✅ | ✅ 落（正文 + 「断了」标记） | 正常 + `error_kind` | 正常的半句话 |
| 危机拦截 / 规则路由 | ✅ | ✅ | 现状不动 | 现状 |

好处：`dialogue_history` / digest / `dialogue_bubbles` 三处**零改动**就自动免疫（它们都按 `kind='dialogue'` + `role` 过滤）。代价：回放里会看到「说了没人答」的一句——这恰好是事实。

### 5.4 工具层：分类 + 副作用措辞

- `should_retry(err, access, name)` → `should_retry(&Failure, access, name)`：`Read && kind.retryable && !web_semantic_failure`。
- `web_search` / `web_fetch` 从「永不重试」放宽到「**传输层**失败重试 1 次」（`Transport` / `Timeout`），语义失败（空结果、抓取不到正文）仍不重试。
- Write 档失败时，回传给模型的文案固定为：「执行失败（该操作可能已部分生效，不要重复调用）：<脱敏原因>」——**明确告诉模型不要重试**，比指望它自己克制可靠。
- 文案进 trace 的 `tool_call` output（现状已存 error，保持）。

> **落地细节（T1 分类）**：`mcp_unreachable` 归 `Transport`（连接层失败，
> Read 档允许 1 次重试，与 T4 现状一致）；`Unavailable` 只留给
> `tool_not_found` / `not_configured` 这类「重试一百次也一样」的错误。
> 陌生错误串按 `Internal` 处理——**不把不认识的错误当成抖动**。

### 5.5 作业层：JobGuard

```rust
// failure.rs
pub struct JobGuard { name: &'static str }
impl JobGuard {
    pub fn should_run(&self, conn: &Connection, now: DateTime<Local>) -> bool;
    pub fn record_ok(&self, conn: &Connection);
    pub fn record_fail(&self, conn: &Connection, f: &Failure); // 写退避状态 + 审计
}
```

- 状态存 `settings`（KV，不建新表）：`job_backoff:consolidate = {"fails":2,"next_at":"2026-11-23T03:40:00+08:00"}`。
- `record_fail` 按 `Backoff{base:60s, cap:30min}` 计算 `next_at`；`fails >= 5` 直接把 `next_at` 推到明天 03:00。
- 成功后清 key。审计行补 `status='error'` + `error` 列（v17），「昨晚为什么没整理」从此可查。
- **不改** `decide_trigger` 的纯函数签名（它已有单测），退避判断放在 `tick` 里、`decide_trigger` 之后——两件事分开：**该不该跑**（纯函数）与**能不能跑**（退避）。

### 5.6 进程与资源

| 场景 | 现状 | 方案 |
|---|---|---|
| sidecar 运行期崩溃 | 无感知 | 每次模型失败先 `probe.is_up()`；连续 2 次 down 且进程 `try_wait()` 已退出 → `serve::restart()`（复用 `attempt_start`），每小时至多 1 次；期间 emit `llm:restarting`，前端在气泡上写「本地模型在重启」 |
| DB 打不开 | `setup` Err → 闪退 | 见 D5：改名备份 + 建空库 + 设 `settings.db_recovered_at`；前端横幅一次性告知。**不静默**：必须让用户知道「记忆是空的，旧库在这儿」 |
| Mutex 中毒 | 永久 sqlite 错误 | `lock_conn` 已是单点（`dialogue.rs:286`）：中毒时返回 `Failure::Internal`，命令层统一回 `internal_poisoned`，UI 提示「需要重启应用」并给「重启」按钮（`tauri::process::restart`）。不做 unsafe 恢复 |
| 后台循环 panic | 线程永久停摆 | 7 个 `spawn` 统一走 `failure::spawn_guarded(name, f)`：`catch_unwind` + `log::error!` + 继续下一 tick；同一线程连续 5 次 panic → 停止重试并写一条 `error` 审计（防止 panic 风暴刷日志） |
| 启动残留 `running` | 无 | 启动时把 `tasks.status='running'` 且 `created_at < now-24h` 的行标 `interrupted`；`turn_trace` 无 `assistant_event_id` 且超 24h 的轮不参与任何统计（不删数据） |

---

## 6. 数据与观测（schema v17）

| 变更 | 用途 |
|---|---|
| `turn_trace.error_kind TEXT` | 这一轮为什么失败（可 GROUP BY 出「最近一周失败分布」） |
| `turn_trace.attempts INTEGER` | 重试了几次（含成功前的失败次数） |
| `consolidate_runs.error TEXT` | 夜间作业失败原因（`status='error'` 已在列定义里，只是从没写过） |
| `settings` 新 key `job_backoff:*` | 退避状态，不建表 |

不改：`events`（失败轮不落 assistant 行，无新列需求）、`trace_steps`（`retry` 枚举早已存在，`dialogue.rs:1053` 就在用）。

**trace 呈现**：`retry` 步的 `output` 记 `kind=Transport delay=500ms attempt=1/2`；`/trace` 页 `TraceTimeline` 已有 `retry` 的 tone（`TraceTimeline.tsx:26`），无需改版式。

---

## 7. 前端

### 7.1 终态事件结构

```ts
// 旧：{ turn_error: "string" }
// 新：{ turn_error: { kind, message, retryable, detailId? } }
```

后端保留 `turn_error` 字段名，载荷从 `String` 变成对象（v1 内没有第三方消费者，直接切换；`useChat` 同步改）。

### 7.2 失败气泡

`ChatMessage` 加一个字段（`chatStore.ts:18-36`）：

```ts
failure?: { kind: string; text: string; retryable: boolean };
```

渲染（`MessageBubble` 下方一条细行，不占正文）：

```
──────────────
这一轮没等到结果：云端出故障了（不是你的问题）        [重试]
```

- 只有 `retryable === true` 才出「重试」按钮。
- 正文取长规则：`outcome.reply` 是兜底句而本地已有流式文本 → **保留本地文本**，只追加 `failure`（修 §1 的覆盖事故）。
- 失败气泡不参与 `noteFor` 的表情联动（不装成功）。

### 7.3 重试语义

`dialogueChatStream(text, sessionId, { retryOf: userEventId })`：

- 后端看到 `retryOf` → **不插新 user 事件**，直接复用那条；`turn_trace` 新建一行（`user_event_id` 相同）——同一句话的两次尝试，历史里只出现一次。
- 前端：移除失败气泡 → 重新开一个 streaming 气泡 → 用户气泡原地保留（不新增）。
- 幂等：点两次「重试」第二次被 `chattingRef` 挡住（现状已有），不需要额外去重。

### 7.4 后台失败

不弹 toast（现在 `toast` 1.9s 自动消失，说了等于没说）。统一进设置页「诊断」区：最近 10 条失败（kind / 时间 / hint），数据来自 `turn_trace.error_kind` + `consolidate_runs.error`。本版只做只读列表，不做重跑按钮。

---

## 8. 端到端流程（失败一轮的完整生命周期）

```
用户发送
  └─ 模型请求 ──失败(Transport)──→ Backoff 500ms → 重试 1 次
        │                                   └─失败→ Failure{Transport}
        ├─ 云端循环：请求 3 失败，但已跑 2 个工具
        │     └─ 重发同一 msgs（不重跑工具）→ 失败
        │           └─ 收尾请求（无 tools）→ 成功：「我查到了 A、B，C 那步没做完」
        │                 └─ 落库：user + assistant（真实收尾）+ trace(error_kind=Server, attempts=2)
        └─ 全部失败
              └─ 落库：仅 user（无假回复）+ trace(error_kind=Internal)
                    └─ 前端：保留已流出文本 + 失败行 + 「重试」
                          └─ 点重试 → 复用 user 事件 → 新 trace 行
```

---

## 9. 分期（逐可编译单元提交）

| 期 | 内容 | 门禁 |
|---|---|---|
| **T1** | `failure.rs`：`Failure` / `FailureKind` / 三个 `from_*` / `Backoff` / `JobGuard` 的纯函数部分；**零接线** | `cargo test --lib failure::` 全绿；`cargo check` 无新告警 |
| **T2** | `DialogueBackend::take_failure` + `LocalBackend` 存错误与 `got_delta` + 模型层重试（2 次，按 kind） + `llm_round` output 带 detail | 既有 541 个单测不受影响（默认 `take_failure`→None）；新增 A1–A4 |
| **T3** | 轮层恢复：云循环不丢成果、重发同 msgs、收尾请求、本地拼装兜底、失败轮落库规则（§5.2/5.3） | A5、A11；`dialogue.rs` 既有 cloud 测试全绿 |
| **T4** | 工具层：`should_retry(&Failure)`、web 传输失败 1 次、Write 副作用措辞 | A6、A7 |
| **T5** | 作业层：`JobGuard` 接线 `scheduler::tick`、v17 迁移（两列 + 一列）、`spawn_guarded` 替换 7 个 `spawn` | A8、A9 |
| **T6** | 进程层：sidecar 看门狗、DB 第二出口、中毒提示、启动清 `running` | A10；真机拔网/杀 sidecar 各验一次 |
| **T7** | 前端：`turn_error` 结构化、失败气泡、重试、取长规则、诊断列表 | A12；`tsc` + `vitest`；截图核对 |

> 顺序理由：T1 是纯函数（可先落地、零风险）；T2/T3 是收益最大也最容易出错的两层，先做；T5/T6 动的是后台与启动路径，放后面；T7 最后，因为它依赖 T2 的载荷形状。

---

## 10. 验收标准

| # | 场景 | 通过条件 |
|---|---|---|
| A1 | 云端 401 | 不重试；`hint` 指向 key/额度；trace `error_kind=Auth` |
| A2 | 云端 429 | 重试 1 次（退避 ≥500ms）；仍失败 → `RateLimit`；trace 两次 `llm_round` |
| A3 | 云端 502 | 重试 1 次；仍失败 → `Server`；用户看到「云端出故障」，不是「脑子转不动」 |
| A4 | 流式中途断（已发 100 字） | **不重试**；正文保留；气泡尾附「说到一半断了」；`error_kind=Interrupted` |
| A5 | 云端循环第 3 次请求 502，此前已跑 2 个工具 | 任务卡保留 2 步；扣 1 次重发；若仍失败 → 收尾请求产出「做到了什么」；**不出现**兜底句 |
| A6 | Write 工具失败 | 不重试；回传文案含「可能已部分生效，不要重复调用」 |
| A7 | Read 工具传输失败 | 重试 1 次；trace 有 `retry` 步；成功则 `retry` 步 `ok` |
| A8 | 巩固作业失败 | `consolidate_runs` 新增一行 `status='error'`；`next_at` 生效：60s 后 tick **不**再跑；`fails=5` 后当天不再跑 |
| A9 | 后台循环内 panic | 线程存活：下一 tick 正常执行；日志一条 `error`（含 panic 信息）；连续 5 次后停止 |
| A10 | DB 文件损坏（手工截断） | 应用能起来；旧库被改名保留；横幅提示；`settings.db_recovered_at` 有值 |
| A11 | 一轮完全失败后发下一句 | 上一轮的兜底句**不出现**在 system/历史里；digest 不含它 |
| A12 | 点「重试」 | 不产生第二条 user 事件/气泡；新 trace 行 `user_event_id` 与上轮相同 |

---

## 11. 风险矩阵

| 风险 | 概率 | 影响 | 缓解 |
|---|---|---|---|
| 重试把单轮从 28s 拖到 90s，撞前端看门狗 | 中 | 中 | 模型层总预算 ≤ 3 次请求 × 120s 上限本就由 `CHAT_TIMEOUT_SECS` 约束；`IDLE_TIMEOUT_MS=150s` 需同步抬到 240s（T2 一并改，A4 覆盖） |
| 云循环重发导致重复计费 | 中 | 低 | 只在失败后重发 1 次；trace `attempts` 可见；不做「无限续命」 |
| 失败轮不落 assistant → 回放里「有问无答」 | 高 | 低 | 这是诚实状态；气泡可点「重试」（当轮）/「查看详情」看失败原因（重启后） |
| `take_failure` 旁路漏清理 → 失败串到下一轮 | 中 | 高 | 取走即清空 + 每轮开头 `let _ = backend.take_failure();` 显式丢弃残留；单测锁 |
| `spawn_guarded` 吞掉 panic 让 bug 更难发现 | 中 | 中 | panic 一律 `log::error!` 带 backtrace 摘要，并计入启动后的「诊断」计数 |
| DB 自愈误判（其实是文件锁） | 中 | 高 | 只有 `SQLITE_CORRUPT`/`SQLITE_NOTADB` 才走自愈；`SQLITE_BUSY` 先重试 3 次（1s/2s/4s）再报错 |
| v17 迁移与另一条线（v16 S2）冲突 | 低 | 中 | 迁移按 `user_version` 顺序执行；本次只加列（`add_column_if_missing`），不改既有表结构 |

---

## 12. 待裁决决策点

| # | 问题 | 选项 | 推荐 |
|---|---|---|---|
| **D1** | 失败轮的 assistant 侧落不落库 | A 落但打标（`kind='dialogue_failed'`，三处查询加过滤）/ B 不落（只留 user） | **B**：零改动免疫三处查询；回放见「有问无答」是事实 |
| **D2** | 重试是否复用原 user 事件 | A 复用（`retryOf`）/ B 重新插一条 | **A**：否则历史里同一句话出现两次，digest 也会记两行 |
| **D3** | 各层重试次数 | 见 §4.2 表 | 按表执行；模型 2 次、工具 1 次、作业 5 次/天 |
| **D4** | 401/429 是否自动回落本地模型 | A 自动回落 / B 明确报错让用户决定 | **B**：静默换模型会让人格表达突变，用户不知道谁在说话；回落是设置页的动作 |
| **D5** | DB 打不开怎么办 | A 改名备份 + 建空库 + 横幅 / B 直接退到托盘并提示备份路径 | **A**，但必须保留备份文件名在提示里，且只对 `CORRUPT/NOTADB` 生效 |
| **D6** | sidecar 运行期要不要自动重启 | A 自动（1 次/小时）/ B 只提示 | **A**，且只在 `try_wait()` 确认进程已退出时——避免与「端口被别的服务占着」混淆 |
| **D7** | 前端是否给「重试」按钮 | A 给（仅 retryable）/ B 只提示 | **A**；用户重打一遍字的成本远高于点一下 |
| **D8** | 是否新建 `failures` 表 | A 建 / B 用 trace + 审计 | **B**：trace 已有一行一轮，GROUP BY `error_kind` 就够；多一张表就多一处遗忘级联 |

---

## 13. 实施记录

| 期 | 提交 | 门禁 | 说明 |
|---|---|---|---|
| T1 | 本提交 | `cargo test --lib` **552 passed / 1 ignored**（+10 `failure::`）；`cargo check` 无新告警 | 新增 `src-tauri/src/failure.rs`：11 个 `FailureKind`（含 `retryable` / `hint` / `parse`）、`Failure::{new, downgrade, from_llm, from_tool, from_db}`、`is_corrupt_db` / `is_busy_db`、`Backoff`（splitmix64 确定性抖动、cap 为硬上限）与五层预算常量、`JobState`（`after_fail` 第 5 次推到明天 03:00）。零接线 |
| T2 | 本提交 | `cargo test --lib` **559 passed / 1 ignored**（+7 `persona::eval`：A1–A4 模型层）；`tsc` 0 错、`vitest` 187；`cargo check` 仅既有 dead_code | `LlmError::Status` 带 body 摘要（三处传输齐改，401/502 可区分）；`DialogueBackend` 加默认方法 `take_failure` / `attempts`（8 个 impl 零改动）；`LocalBackend` 存 `last_failure`，非流式重试 ≤2 次、流式仅零 delta 重试 1 次、断流交回已流出正文且标 `Interrupted`（不可重试）；`dialogue.rs` 直答收敛进 `call_backend_with_trace`（调用点开头清残留失败，output 记 `kind=… attempts=… detail…`）；前端 `IDLE_TIMEOUT_MS` 150s→240s；既有 `trace_dead_model_still_records_empty_round` 断言同步升级（answered→error + kind=transport，语义升级非回归） |
| T3 | 本提交 | `cargo test --lib` **562 passed / 1 ignored**（+3：A5 三段式、重发恢复、A11 落库）；`tsc` 0 错、`vitest` 187 | 云循环失败分支从「整轮放弃」改三段式（§5.2）：①可重试 kind 且预算有余 → 重发同一 msgs（纯读不重跑工具）②仍失败且已执行工具 → 收尾请求不带 tools ③连收尾也失败 → 本地拼装「做到了 X/Y + hint」（A5：任务卡保留已完成步，兜底句不出现）；失败轮落库（§5.3/D1）：`persist_model_turn` 单点接线两处落库段——完全失败不落 assistant 事件（兜底句只回 UI，A11：历史/digest 零污染），trace `reply_text` 空正文传 None、error 步带 `kind=… attempts=…`；反谄媚过滤与 disagreement 事件只对真正文执行 |
| T4 | 本提交 | `cargo test --lib` **565 passed / 1 ignored**（+3：web 传输重试、web 非传输不重试、A6 Write 措辞 + A7 trace 步）；`tsc` 0 错、`vitest` 187 | `should_retry(err,…)→should_retry(&Failure,…)`：分类只此一处（`Failure::from_tool`），Read 档看 `kind.retryable`，陌生错误归 Internal 不再被当抖动重试；web 工具从「永不重试」放宽到传输层失败重试 1 次（Transport/Timeout，原 P2 裁决被推翻并更新对应测试）；新增 `tool_failure_reply`：Write（Ask）档失败回传固定带「该操作可能已部分生效，不要重复调用」（denied/bad_args 等未生效失败不带）；trace `retry` 步照记（pending 发起 / ok 成功） |
| T5 | 本提交 | `cargo test --lib` **571 passed / 1 ignored**（+6：JobGuard 退避 4、A8 失败审计 2）；前端门禁随合并后统一跑 | `JobGuard`（settings KV，`job_backoff:<name>`）接线 `scheduler::tick`：退避门放 `decide_trigger` **之后**（该不该跑/能不能跑分开，§5.5）；`record_consolidate_failure` 落 `consolidate_runs status='error' + error` 行并 `record_fail`（60s 起步，第 5 次推明天 03:00），成功 `record_ok` 清零；v17 迁移（**SCHEMA_VERSION 16→17，user_version 同步 bump**）：`turn_trace.error_kind/attempts` + `consolidate_runs.error` 三列，`TraceRecorder` 从 error llm_round output 解析 `kind=/attempts=` 落列；`spawn_guarded` + `PanicGuard`（逐 tick catch_unwind，连续 5 次停循环）替换生产 spawn——计划写 7 处，实为 6 处（consolidate.rs:873 是测试桩、serve.rs:239 是进程 spawn），全部替换 |
| T6 | | | |
| T7 | | | |

---

## 14. 变更记录

| 版本 | 日期 | 变更 |
|---|---|---|
| v0.1 | 2026-11-23 | 初稿：现状 17 条实测锚点、`Failure` 分类法、分层策略表、T1–T7、A1–A12、D1–D8 |
| v0.6 | 2026-11-24 | T5 落地：`spawn_guarded` 实际只换 6 处（7 处含两处测试/进程 spawn，勘误）；`error_kind/attempts` 由 TraceRecorder 解析 error 步 output 得来，trace 查询行（TurnTraceRow）暂不加列（诊断列表 T7 用原生 SQL）；scheduler 的 `record_ok` 放 consolidate 成功后、digest_pass 之前——摘要重写失败不清退避 |
| v0.5 | 2026-11-24 | T4 落地：web 重试放宽是**推翻原 P2 裁决**（DDG 9s 延迟顾虑让位于传输抖动恢复），`external_network_tool_failure_is_not_retried` 拆成「传输层重试 1 次」+「非传输层永不重试」两测；Write 档措辞对 denied/bad_args 例外（未生效不谎报副作用） |
| v0.4 | 2026-11-24 | T3 落地：轮层恢复三段式限定「重发 1 次 + 收尾 1 次」的硬预算（重发还受 `requests < CLOUD_REQUEST_CAP` 门控）；`error_kind` 落在 trace 的 error 步 output（`kind=…`），不新增 schema 列；`flaky_backend` 测试桩可按脚本重放 `Result<ToolReply, LlmError>` |
| v0.3 | 2026-11-24 | T2 落地：`Failure` 加 `attempts` 字段（`trace.attempts` 的来源，成功路径从 `backend.attempts()` 读）；流式部分成功（已流出正文）走 `Interrupted` 而非原 kind；`LlmError` 变 `Clone`（测试桩按脚本重放） |
| v0.2 | 2026-11-23 | T1 落地：`Backoff` 的 cap 改为硬上限（抖动不许越过）；`mcp_unreachable` 归 `Transport` 而非 `Unavailable`（连接层失败该重试 1 次，与 T4 现状一致，见 §5.4 落地细节） |

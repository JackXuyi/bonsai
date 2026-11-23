# 方案：perception_search —— 感知事件的 AI 查询工具（内置工具第二枚）

> **状态**：M1–M2 已实施（2026-11-23，cargo 385 单测全绿，clippy 回基线）；M3 真机验收待做（`pnpm app`）
> **日期**：2026-11-23
> **依赖**：内置工具机制（memory_search，已落地）；云端 DeepSeek function calling（T1–T6，已落地）；感知采集（perception.rs，已落地）
> **背景**：2026-11-23 真机发现：最近事件页能看到 perception 事件（app / clipboard / download），但模型没有任何工具能查它们——「我今天在用什么应用」「我刚才复制了啥」答不上，与 2026-11-20 memory_search 立项前的「18 号说了啥」是同一类盲区。

---

## 1. 目标与非目标

### 目标

1. 新增内置工具 `perception_search`（`ToolKind::Builtin`）：把 `events WHERE kind='perception'` 的查询权交给 AI，单一权威实现。
2. 输出为**聚合视图**（不是逐行回放）：app 使用统计、剪贴板存在性（隐私预览）、下载清单。
3. 双路径接线，与 memory_search 完全同构：
   - 云端路径：`tools[]` 挂载，模型原生 function calling 调用；
   - 本地路径：**代码层强制路由**（`might_perceive` 信号命中 → 引擎代调 → 人格轮措辞），本地模型零工具调用，符合 2026-11-22 硬约束。
4. 日期窗解析复用 `memory_search::parse_window`（消费，不另写一份）。

### 非目标（明确不做）

- **不回放剪贴板原文给模型**：剪贴板是密码/验证码/敏感文本的高发地。模型侧只给预览（≤24 字符）+ 次数；`full`（/trace 页，用户本人查看）给存储的截断值（写入时已 clip 到 1000 字符）。
- 不查派生事件（`face_state` / `growth_stage` / `relationship_*`）：那是盆栽自己的状态，不是用户环境；工具 SQL 钉死 `kind='perception'`。
- 不做「此刻在用什么 app」的实时通道：P0 用最近窗口近似（perception 采样间隔 15s，最近事件距此刻 ≤ 几分钟）。实时信号（`LatestSignals`）进对话上下文属 P3 屏幕感知范畴。
- 不改 schema（v12 不动）、不加设置项（感知源开关已有，挂载跟随采集开关语义——见 §6）。

---

## 2. 现状盘点（写方案前的代码事实）

| 已有 | 位置 | 说明 |
| --- | --- | --- |
| 感知事件落库 | `perception.rs::spawn`（L358） | `add_event(conn, "perception", content)`；content 形态 `app: X` / `clipboard: …`（clip 1000 字符）/ `download: name (N bytes)`；采样间隔 15s，Collector 去重 |
| 日期窗解析 | `memory_search::parse_window`（已 pub） | 今天/昨天/周X/N天前/M月D日……全谱系；`recall_in_window` 显式窗协议 |
| 内置工具注册 | `agent_tools::builtin_tools()`（L43） | 单源 registry，设置页与模型目录同源 |
| kit 挂载门控 | `mcp_admin::build_agent_kit`（L684） | `might_recall(user_text)` → 挂 MemorySearchProvider |
| 本地强制路由 | `dialogue.rs::local_recall_answer`（L595） | recall 信号 → 引擎代调 memory_search → pass-3 措辞；失败回落直答 |
| 云端循环 | `dialogue.rs::cloud_tool_loop` | kit.catalog → tools[]；工具 ≤8 / HTTP ≤6 护栏；失败脱敏回传 |
| 感知消费方① | `perception::decision_input` | idle/in_meeting → should_speak，模型不可见 |
| 感知消费方② | `memory::db::app_rhythm`（L839） | `app: %` 直方图喂 RulePredictor 规则② |

| 缺口 | 说明 |
| --- | --- |
| 模型查询通道 | 无任何工具能查 perception 事件；memory_search SQL 写死 `kind='dialogue'` |
| 抽取通道 | consolidate 只吃 dialogue，感知事件不进 facts，本地纯聊天连间接记忆都没有 |

---

## 3. 工具设计

### 3.1 签名（与 memory_search 同构）

```json
{
  "name": "perception_search",
  "summary": "查一段时间内你在用什么应用、复制过什么、下载过什么（本地，聚合视图）",
  "detail": "{\"type\":\"object\",\"properties\":{\"query\":{\"type\":\"string\",\"description\":\"用户原话，日期由系统解析；给了 date_from/date_to 时可省\"},\"date_from\":{\"type\":\"string\",\"description\":\"显式开始日期 YYYY-MM-DD（含）\"},\"date_to\":{\"type\":\"string\",\"description\":\"显式结束日期 YYYY-MM-DD（含）\"}}}"
}
```

- `access: ToolAccess::Read`、`remote: false`、`needs_conn: true`——与 memory_search 相同，走 `execute_with_conn`。
- 模型只回传 query 原话（或显式日期对），**解析全在 Rust 侧**——沿 memory_search 的裁决：2B/工具调用模型都不许参与日期解析。
- `query` 与 `date_from/date_to` 二选一；显式日期优先；query 解析不出日期 → 默认窗口是**最近 24 小时**（有意偏离 memory_search 的最近一周：感知事件一天可达数百条，周窗聚合出来是一锅粥；「我昨天在用什么」才是高频问法）。

### 3.2 聚合视图（模型侧 summary）

SQL 在库侧聚合（事件量大，禁止全量捞回 Rust）：

```sql
-- app 维度：次数 + 首末时间（Rust 侧再转本地时间显示）
SELECT content, COUNT(*) AS n, MIN(ts), MAX(ts)
FROM events
WHERE kind = 'perception' AND content LIKE 'app: %'
  AND ts >= ?1 AND ts < ?2
GROUP BY content ORDER BY n DESC LIMIT 20;

-- clipboard / download：同构，LIKE 前缀换成 'clipboard: %' / 'download: %'
-- clipboard LIMIT 20、download LIMIT 30
```

summary 输出形态（行式，沿 memory_search 的 2B-prose 裁决）：

```text
【11月23日（周一）感知】应用 6 个、剪贴板 3 次、下载 2 个
应用：微信 ×23（09:12–10:13）、Cursor ×41（09:30–10:12）、bonsai ×8…
剪贴板：3 条（10:10 一条以「ItemLink」开头…）   ← 预览 ≤24 字符，见 §3.3
下载：design-v2.pdf (2.3 MB)…
```

- 预算：`SUMMARY_TOTAL_CHARS = 800`（感知聚合天然短，无需 memory_search 的 1600）。
- 空窗 → 诚实「那段时间没有感知记录（感知开关没开，或电脑没在用）」——区分「没记录」与「没开采集」，不许用前者掩盖后者（沿日历线 permission_message 的先例：不拿空答案冒充事实）。感知三源全关时直接说明。

### 3.3 隐私裁剪（硬约束，代码强制）

| 维度 | 模型侧（summary） | /trace 页（full JSON） |
| --- | --- | --- |
| app | 完整应用名 + 次数 + 首末时间 | 同 summary，逐条明细 |
| clipboard | **条数 + 时间 + 预览 ≤24 字符**（首行截断） | 存储值（写入时已 clip 1000），只进用户本人可见的 trace |
| download | 完整文件名 + 大小 | 同 |

- 代码注释记 deviation 理由：剪贴板原文进模型上下文 = 密码/验证码泄入对话与后续云端请求，不可接受；预览保住「刚才复制的是哪个链接/哪段话」的可用性。
- `full()` 沿 2026-11-21 裁决：JSON 结构化，前端 /trace 渲染。

### 3.4 关键词过滤（可选腿）

query 剥掉日期短语与脚手架词后剩余关键词（复用 `memory_search::keywords`，需将其 pub——当前私有，改可见性不动逻辑）：

- app 维度：`content LIKE '%kw%'` 过滤（「我在 Cursor 上干了啥」）；
- clipboard 维度：对预览段 LIKE 过滤；
- 无关键词 → 整窗聚合（「我今天在用什么」）。

---

## 4. 挂载门控：`might_perceive`

新文件 `perception_search.rs` 内置独立门控（**不并入 might_recall**——词表语义不同，混表会让「我今天好累」类误挂载变多；两个门各管各的，挂载成本一次 provider 注册）：

```rust
const PERCEIVE_MARKERS: &[&str] = &[
    "在用什么", "用了什么", "用的什么", "开着什么", "哪个应用", "什么应用", "什么软件",
    "用了哪些", "打开过", "切过", "剪贴板", "复制过", "粘贴", "拷贝",
    "下载过", "下载了", "看过什么", "浏览过",
];
```

- 命中条件：词表任一命中 **或** `might_recall` 同款日期短语检测命中**且**含「用/开/剪贴板/下载」类动词（避免「18 号」裸日期把感知工具也挂上）。
- 误报成本 = 一次工具注册 + 可能一次模型轮（模型可不调），沿 might_recall 的「宁滥勿缺」取舍。
- **采集开关联动**：`PerceptionConfig.enabled == false` 且三源全关时，provider 仍可挂载（模型需要知道为什么答不了），execute 返回「感知没开，工具页里打开我才能看到」类脱敏说明 + `tool_calls` 落 `denied` 行——沿日历线「解释而非空答案」。

## 5. 双路径接线

### 5.1 云端（主路径）

`mcp_admin::build_agent_kit` 加一段（L684 旁）：

```rust
if crate::perception_search::might_perceive(user_text) {
    providers.push(Box::new(crate::perception_search::PerceptionSearchProvider));
}
```

- `tool_specs` 自动进 `tools[]`；`tool_label` 前缀 `builtin:perception_search`（现有 kind 前缀逻辑自动覆盖）。
- 前端 `noteFor` 认 `builtin:` 前缀——自动生效，无前端改动。

### 5.2 本地（代码层强制路由，模型零工具调用）

`dialogue.rs::local_recall_answer` 旁新增 `local_perception_answer`（同构函数，模式裁决沿 v0.2 §5 D1）：

1. kit 无 `perception_search` → None（回落直答）；
2. `might_perceive` 未命中 → None；
3. 引擎代调 `execute_audited(store, kit, "perception_search", args)`（args 构造同 `memory_search_args`：query 原话透传）；
4. 成功 → 走**同一条** `summarize_tool_result`（pass-3 措辞，不复制第二份）；
5. 失败 → 审计行已落，回落直答，不静默。

调用点与 `local_recall_answer` 相邻接线：两个门控独立判断（一回合可同时命中 recall + perceive，先 recall 后 perceive，各自独立回落）。

### 5.3 注册

`agent_tools::builtin_tools()` 追加 `crate::perception_search::builtin_descriptor()`——设置页「内置工具」自动出现，registry 单源测试（`builtin_registry_matches_provider`）同步更新预期条数。

---

## 6. 边界与不变式

| 不变式 | 保障 |
| --- | --- |
| 本地模型零工具调用 | 本地路径只有引擎代调（代码层强制路由），不发 tools[]、无 JSON 协议 hack |
| 能力单份实现 | 感知查询只在 perception_search.rs；app_rhythm（预测）与 decision_input（开口决策）是既有消费方，不冲突 |
| 派生事件不进 | SQL 钉死 `kind='perception'`，face_state/growth/relationship 天然排除（沿 morph 线教训） |
| 隐私 | 剪贴板模型侧预览 ≤24 字符，代码常量 + 单测锁死 |
| 审计 | 每次执行落 `tool_calls`（kind=builtin），含 denied/error 行 |

---

## 7. 里程碑拆分

| 里程碑 | 内容 | 验收 |
| --- | --- | --- |
| M1（0.5 天） | `perception_search.rs`：门控 + 窗口解析（复用 parse_window/keywords，pub 化）+ SQL 聚合 + 隐私裁剪 + registry 注册；`cargo test` 全绿含新增单测（门控正/负例、窗口、聚合、剪贴板预截断、关采集的 denied 路径） | cargo 335+ 全绿；registry 测试更新 |
| M2（0.5 天） | `build_agent_kit` 挂载 + `local_perception_answer` 接线 + dialogue 集成测试（云端 mock tool_calls 全链路、本地强制路由 trace `local:forced-perception-search`、失败回落） | cargo 全绿；vitest 不动（前端零改动） |
| M3（0.5 天） | 真机验收 + 文档回填（本文件状态改「已实施」+ working memory 记录） | `pnpm app` 真机：云端模式问「我今天在用什么应用」命中工具；本地模式同问走强制路由且人格轮措辞正常；/trace 页看到 full JSON |

---

## 8. 风险与开放问题

| 风险 | 应对 |
| --- | --- |
| 感知事件量极大（一天 1282 条） | 聚合全在 SQL（GROUP BY + LIMIT），Rust 只拿聚合行；无全量捞回路径 |
| 剪贴板预览仍可能泄敏感（24 字符已含密码前缀） | 接受残余风险：预览可辨认「是哪个链接/哪段话」，24 字符内完整密码概率低；后续可加「纯数字/高熵串打码」规则（P1 再议，不在 P0） |
| 与 memory_search 双双挂载时 tools[] 膨胀 | 工具 ≤8 护栏现值 1(memory)+4(fs)+MCP，加 1 个余量充足；护栏触发走既有强制收尾 |
| might_perceive 词表漏词（「刚才在忙什么」不命中） | 与 might_recall 同哲学：漏词是迭代教训驱动（memory_search 的「原文/逐字」先例）；真机验收后按实际漏答补词 |
| 「现在在用什么」实时性 | P0 以最近窗口近似；采样间隔 15s，误差 ≤ 数分钟，可接受；实时信号进上下文归 P3 屏幕感知 |

**开放问题（评审时定）**：
1. `keywords` 从 memory_search pub 化 vs 在 perception_search 内复制一份小型关键词剥离——已定并落地：pub 化（`keywords_with(text, date_phrase, extra_scaffolding)`），perception 传入自己的脚手架词表；**extra 词先于通用词剥离**（通用表含「什么」，会把「在用什么」截胡成残留词「在用」）。
2. app 维度要不要估算「使用时长」——P0 不做（计数 + 首末时间已可答绝大多数问题），P1 再议。

---

## 9. 实现注记（2026-11-23 落地时的裁决与坑）

| 项 | 实际落地 |
| --- | --- |
| 文件 | 新增 `src-tauri/src/perception_search.rs`（含全部单测）；`lib.rs` 注册模块 |
| pub 化清单 | `memory_search::{normalise, has_date_signal, keywords_with, iso_pair, window_label, local_stamp}`——感知侧全部消费，零复制 |
| 采集开关探测 | 连接里拿不到 cfg path → `PRAGMA database_list` 取 main 库文件路径 → 同目录 `perception.json`；**内存库（测试）视作开启**。开关关 → Ok 返回「感知采集没开」说明（未落 denied 行，沿 Ok+解释实现，模型据此向用户转述；偏离原 §4 的 denied 行设计，理由：execute_audited 对 Ok 输出没有 denied 通道） |
| 24h 兜底 | 无日期短语 → `[now-24h, now)`，label「最近 24 小时」 |
| might_perceive 日期半边 | 用 `parse_window().is_some()`（覆盖 昨天/今天/周X/M月D日 全谱系），不用 `has_date_signal`（只认数字日期+星期，「昨天在用啥」会漏） |
| 关键词过滤 | Rust 侧 `to_lowercase()` 双侧折叠——SQLite LIKE 的 ASCII 不敏感这里没有；normalise 去空格会把「在用 Cursor」黏成「在用cursor」→ 脚手架词表补「在用/用了/用的/开着/用过」 |
| 总数统计 | `count_rows` 独立 COUNT 查询（pre-LIMIT）——对 LIMIT 截断后的聚合行求和会少报密集窗口 |
| 本地强制路由 | `dialogue.rs::local_perception_answer`，与 local_recall_answer 同构；`reborrow` 改为 `&mut reborrow` + `as_mut().map(\|f\| &mut **f)` 以支持两段路由共享流式回调 |
| 云端/本地集成测试 | `builtin_perception_search_full_chain_is_audited_and_labeled`、`local_perception_route_executes_perception_search_with_persona`、`kit_gates_memory_tool_by_recall_signal` 扩展感知门断言（裸日期不挂感知工具） |
| 剪贴板预览 | 24 字符 + 「…」单测锁死；full JSON 里的全文只进 /trace 页 |
| 测试规模 | cargo 385 全绿（新增 9 个感知侧单测 + 2 个 dialogue 集成）；clippy 36 条 = 基线，零新增 |
| 前端 | 零改动（`builtin:` 标签自动生效） |

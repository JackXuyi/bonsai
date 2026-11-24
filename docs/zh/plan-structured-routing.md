# plan-structured-routing — 原生结构化输出 + 自动大小模型路由

| 文档信息 | 内容 |
|---|---|
| 版本 | v0.2（已实施） |
| 日期 | 2026-11-24 |
| 状态 | **已实施**（T1–T5）；D1–D4 按推荐闭合 |
| 上游输入 | `plan-retry.md`（失败分类）、`plan-task-card.md`（云端工具循环）、spec-m1/m2（配置总线）、AGENTS.md §模型调用约束（2026-11-22 裁决） |
| 定位约束 | 本地 LLM 优先；三级渐进 JSON 管线已退役**不得复活**；本地对话零工具调用；能力不重复实现；宁漏勿误 |
| 一句话 | 让「机器约束的 JSON」替代「提示词纪律的 JSON」，让「后台任务默认不出机器」替代「全腿同源一锅端」 |
| 用户裁决 | **D1** FollowConfig 默认；**D2** 周报/日报亦钉 Background（与 §3.6 文案一致）；**D3** V1 无本地 7B 档；**D4** 新 JSON 消费点直接挂 `chat_structured`。均 2026-11-24 |

---

## 0. 一句话结论

现在全仓的模型调用有两个结构性缺口：

1. **结构化输出靠提示词纪律**。唯一的 JSON 消费点（记忆抽取，`memory/extract.rs`）把格式要求写进 system prompt（`extract.rs:341-351`），靠 `strip_fences`（`extract.rs:371-380`）剥 markdown 围栏后硬解析（`extract.rs:416-418`）。2B 模型偶发输出散文 / 半截 JSON → 整次解析失败 → 当日抽取全部退化为规则层（宁漏勿误，安全但浪费了模型额度）。而 Ollama 早已支持原生 grammar 约束（`format: <json schema>`），云端 OpenAI 兼容协议也有 `response_format`——**传输层能保证的事，不该靠提示词祈祷**。
2. **全腿同源**。`llm::choose()`（`llm/mod.rs:294-332`）一个开关切全部调用点：用户一旦启用云端，夜间抽取 / 摘要重写 / 周报也跟着出机器（隐私 + 费用都随对话腿被动升级）；反过来只配本地时，对话质量被 2B 封顶。**「对话要质量」和「后台要隐私」是两个需求，现在只有一根线**。

本方案补两件事：

1. **原生结构化输出**：`ChatClient` 新增 `chat_structured`（默认 `Err("structured_unsupported")`，复刻 `chat_with_tools` 的类型强制范式，`llm/mod.rs:242-251`）；Ollama 走 `format: schema`（grammar 级约束），云端走 `response_format: {type:"json_object"}`，Anthropic 不支持走兜底。**不做任何「解析失败→换提示词→重试」的渐进修复**——那是已退役的三级管线换皮，失败即走既有的 `RuleExtractor` 兜底。
2. **任务分腿路由**：调用点按「对话（Chat）/ 后台（Background）」分两档。Auto 模式下后台任务**固定走本地小模型**（不出机器、不烧云端费用），对话与工具循环跟随用户主配置；Manual 模式 = 现状逐字节等价。配置落 settings 表，**不升 schema version**。

> 关键认知：**结构化输出不是工具调用**。AGENTS.md「本地零工具调用」约束的是 tools[]/function calling（对话管线挂工具）；夜间抽取本来就是本地的合法职责（plan-memory §5.2），给它 grammar 约束只是把「输出形状」从概率问题变成编译问题，不触碰任何工具挂载路径。方案落地时在 AGENTS.md §模型调用约束补一句澄清，防止后人误判。

### 关键取舍（先看这五条）

| 取舍 | 结论 | 理由 |
|---|---|---|
| 结构化失败要不要重试 / 降级 | **不**。失败即 `RuleExtractor`（现状路径不变） | 三级渐进 JSON 管线 2026-11-22 退役，「解析失败→修复提示词→重试」是它的复活形态；夜间作业宁漏勿误，规则层本来就是设计好的第二条命 |
| Ollama 侧用 `format:"json"` 还是 schema | **schema**（结构化对象，Ollama ≥0.5） | `json` 只保证「是合法 JSON」，不保证字段/枚举；schema 把 category 枚举、profile 形状也压进 grammar，Rust 侧只做语义校验（白名单、idx 越界） |
| 云端用 `json_schema` 还是 `json_object` | **`json_object`**（一次只发一种） | DeepSeek 官方支持 json_object；`json_schema` 各家兼容参差。schema 语义留在提示词 + Rust 校验，grammar 由 json_object 兜住 |
| 「大模型」指什么 | **V1 只指云端激活条目**；本地大模型（7B）档位不做 | 本地多模型分档依赖用户手动 pull + 内存换载体验差，与 roadmap「Computer Use 等 ≥7B」同属缓做；V1 的「路由」= Chat/Background 两腿，不是 N 档模型矩阵 |
| Auto 模式默认开还是关 | **默认 Manual（现状），UI 一键开 Auto** | 行为变化必须显式 opt-in；默认值改了等于静默改隐私边界 |

---

## 1. 现状锚点（全部实测，带行号）

### 1.1 模型腿与工厂

| 现状 | 位置 | 对本方案的约束 |
|---|---|---|
| 三种传输实现同一 trait `ChatClient`：`chat` / `chat_stream` / `is_up` / `label` | `llm/mod.rs:221-251` | 新增 `chat_structured` 走同一 trait + 默认 Err 范式，调用点零感知不支持方 |
| `chat_with_tools` 默认 `Err(LlmError::Decode("tool_unsupported"))`，只有云端传输覆写 | `llm/mod.rs:242-251` | **范式样板**：结构化输出用同一形状（默认 unsupported，类型系统强制） |
| 唯一工厂 `choose()`：cfg.provider=Cloud 且条目存在 → 云客户端；否则 Ollama | `llm/mod.rs:294-332` | 路由分腿不动 `choose`，在其上加 `client_for(TaskLeg)`；条目删除回退本地的既有语义保留 |
| 覆盖层 `LLM_OVERRIDE`，每调用点实时读，切换免重启 | `llm_admin.rs:56-78`、`llm/mod.rs:336-345` | RouteMode 增量进 `LlmConfig`，沿用同一覆盖层，不新增全局状态 |
| Ollama 请求体：`model/messages/stream/think/options{temperature,num_ctx}`，**无 `format` 字段** | `llm/ollama.rs:26-44`、`117-125` | 结构化输出 = 给 `ChatRequest` 加 `#[serde(skip_serializing_if = "Option::is_none")] format: Option<&Value>` |
| 云端请求体 `chat_body`：`model/messages/stream/temperature/max_tokens`，**无 `response_format`** | `llm/cloud.rs:93-107` | 结构化输出 = `chat_body` 加可选 `response_format` 字段 |
| 模型清单已具备（`/api/tags`、tag 比对、删除） | `llm_models.rs:44-79` | V1 不新增「本地大模型档」；清单是将来第三档的基础设施 |

### 1.2 JSON 消费点（唯一）与文本消费点

| 现状 | 位置 | 对本方案的约束 |
|---|---|---|
| `LocalExtractor` 用 `current_client()`（跟随全局腿，含云端）+ `chat` + 提示词纪律 | `memory/extract.rs:303-326`（309 行 `current_client()`） | 迁移到 `chat_structured`；腿的归属由 §3 路由层决定，抽取器自身不感知 |
| 格式靠 system prompt 逐字段描述（`{"idx":...,"fact":...}`） | `extract.rs:341-351` | schema 注册后提示词中的「格式行」删除，规则行保留（第三人称改写、宁漏勿误是语义不是语法） |
| `strip_fences` 剥 markdown 围栏后 `serde_json::from_str` 整体解析，失败 → `None` → `RuleExtractor` | `extract.rs:371-380`、`416-418`、`335-339` | 兜底路径**原样保留**；native 模式下围栏不应再出现，`strip_fences` 保留作云端 json_object 模式的无害冗余 |
| 语义校验全在 Rust：idx 越界丢条、category 枚举丢条、confidence clamp、profile 白名单 | `extract.rs:420-443`、`395-411` | **不变**。schema 只管 grammar，白名单/越界/宁漏勿误继续在 Rust（可测、可锁） |
| 摘要 L2 重写：纯文本输出，剥围栏 + clip | `memory/consolidate.rs:417-437` | 不迁 structured（无结构）；只受 §3 路由影响 |
| 周报/日报：纯文本，失败回退统计摘要 | `summary.rs:642-656`、`report.rs:224/295` | 同上 |
| 对话：persona 流式，失败兜底句（plan-retry 管辖） | `persona/eval.rs:165-190` | 不迁 structured；只受 §3 路由影响 |

### 1.3 隐私与费用相关

| 现状 | 位置 | 对本方案的约束 |
|---|---|---|
| 用户启用云端后，`current_client()` 让抽取/摘要/周报全部走云——发言内容与画像原材料出机器 | `llm/mod.rs:294-332` + `extract.rs:309` | Auto 模式的核心动机：后台固定本地，隐私默认不出机器 |
| 云端 opt-in 警告承诺「显式开启才出机器」 | spec-m2 §4（模块注释 `llm/mod.rs:14-17`） | Auto 模式下开启云端只升级**对话腿**；设置页文案需写明这一点，不与承诺冲突 |
| 云端 `max_tokens` 封顶 4096（防天价账单） | `llm/mod.rs:57-59` | 路由到云的只有对话/工具腿，封顶语义不变 |
| 配置存 settings 表，不升 schema（§A3 先例） | `llm_admin.rs:1-8` | RouteMode 沿用 settings 键值，`user_version` 不动 |

---

## 2. 能力边界

### 2.1 做

| 能力 | 做到什么程度 |
|---|---|
| `chat_structured` trait 方法 | 默认 `Err(Decode("structured_unsupported"))`；Ollama / Cloud 各自覆写；返回原始 JSON 字符串，解析与语义校验留给调用点（与 `chat` 同职责边界） |
| Ollama 原生约束 | `format: <json schema>`（对象形态）；sidecar 不支持 → 该次调用 `Status` 失败 → 既有兜底，**不做版本探测不做双发** |
| 云端 json_object | `response_format: {"type":"json_object"}`；条目不支持 → 该次失败 → 既有兜底；不加能力位、不做降级重试 |
| schema 注册表 | `llm/schema.rs`：常量 JSON Schema（先只有 `EXTRACTION_FACTS`），带版本注释；调用点引用常量，schema 与 Rust 解析结构在单测里互锁 |
| 任务分腿 | `TaskLeg::{Chat, Background}` + `llm::client_for(leg)`；`current_client()` = `client_for(Chat)`（全部既有调用点语义不变） |
| Auto/Manual | `LlmConfig` 增量字段；Manual（默认）= 现状逐字节等价；Auto = Background 恒走本地小模型，Chat/Tools 跟随主配置 |
| 可观测 | 后台腿每次调用 `log::info` 带 `label()`（`ollama:qwen3.5:2b` / `cloud:deepseek-chat`），夜间作业「谁回的」可查 |
| 前端开关 | 设置页模型区一块「后台任务只用本地模型」开关（默认关 = Manual），复用 llm_admin 读写命令 |

### 2.2 不做（并说明为什么）

| 不做 | 理由 |
|---|---|
| 解析失败后的提示词修复 / 重试 / 换模型重发 | 三级渐进 JSON 管线退役令（AGENTS.md §模型调用约束 3）。grammar 约束已把语法失败压到机器级罕见，剩余失败 = 语义问题，Rust 校验 + 兜底更可靠 |
| 本地大模型（7B）第三档 | 内存换载体验差；roadmap 已缓做 ≥7B 场景；`llm_models` 清单是现成地基，将来加档只改路由表 |
| Anthropic 结构化输出 | `/v1/messages` 无原生 schema 字段；tool-use 技巧属于「为 JSON hack 协议」的灰区。Claude 条目下抽取退回规则层——**诚实失败优于聪明 hack** |
| 抽取/摘要改走云端「提升质量」 | 与 Auto 的隐私目标正面冲突；质量问题的答案是换本地模型 tag（用户可操作），不是换隐私边界 |
| 对话管线结构化（让本地 2B 输出 JSON 做路由） | 直接违反「本地零工具调用 + 不复活 JSON 管线」双裁决。路由是**引擎侧**分腿，不是让模型做意图分类 |
| per-entry 能力位（supports_json_object） | 一次失败即兜底已足够；能力位是配置负担，收益趋零 |

### 2.3 边界澄清（写进 AGENTS.md 的那一句）

> 「本地零工具调用」约束的是 tools[] / function calling（对话管线挂工具）。后台管线（夜间抽取/巩固/总结）在本地模型上使用**原生结构化输出**（Ollama `format` / 云端 `response_format`）不构成工具调用，不触发本约束——工具的执行者始终是引擎，结构化输出只是让模型把话说成机器能编译的形状。

---

## 3. 方案设计

### 3.1 T1 · trait 与 schema 注册表（`llm/mod.rs` + 新 `llm/schema.rs`）

```rust
// llm/mod.rs —— chat_with_tools 的镜像范式
pub trait ChatClient: Send + Sync {
    // ... 既有方法不动 ...

    /// 结构化输出：传输层用原生机制约束输出为合法 JSON。
    /// 返回原始字符串（调用点自己解析 + 语义校验），默认不支持。
    fn chat_structured(
        &self,
        messages: &[Msg],
        schema: &serde_json::Value,
        temperature: f64,
        timeout: u64,
    ) -> Result<String, LlmError> {
        Err(LlmError::Decode("structured_unsupported".into()))
    }
}
```

```rust
// llm/schema.rs（新文件）—— schema 常量注册表，一个消费点一个常量
pub const EXTRACTION_FACTS: &serde_json::Value = &serde_json::json!({
    "type": "array",
    "items": {
        "type": "object",
        "properties": {
            "idx":         { "type": "integer", "minimum": 0 },
            "fact":        { "type": "string" },
            "category":    { "enum": ["like", "habit", "context", "boundary"] },
            "confidence":  { "type": "number", "minimum": 0.0, "maximum": 1.0 },
            "correction":  { "type": "boolean" },
            "profile":     {
                "type": ["object", "null"],
                "properties": { "key": {"type":"string"}, "value": {"type":"string"} }
            }
        },
        "required": ["idx", "fact", "category"]
    }
});
```

要点：

- **grammar 与语义的分工**：schema 保证「是数组、元素形状对、枚举合法」；Rust 继续负责 idx 越界、profile 白名单、confidence clamp（`extract.rs:420-443` 原样保留）。两侧在单测互锁：用 schema 过一遍测试桩样本，再用 `RawCandidate` 过一遍，两关都过才算合法。
- `EXTRACTION_FACTS` 单测断言与 `RawCandidate` 字段一致（加 `profile` 缺省即 null），防止两边漂移。

### 3.2 T2 · Ollama 腿（`llm/ollama.rs`）

```rust
#[derive(Debug, Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: &'a [Msg],
    stream: bool,
    think: bool,
    options: Options,
    /// 结构化输出（Ollama ≥0.5）：Some(schema) 时 grammar 约束输出。
    #[serde(skip_serializing_if = "Option::is_none")]
    format: Option<&'a serde_json::Value>,
}

impl OllamaClient {
    pub fn chat_structured(&self, messages, schema, temperature, timeout) -> Result<String, LlmError> {
        let mut req = self.request(messages, temperature, false);
        req.format = Some(schema);
        // post / 解析复用 chat() 的形状
    }
}
```

- sidecar 版本过旧不认识 `format` 对象 → 服务端 400 → `LlmError::Status(400, ...)` → 调用点兜底。**不做版本探测**：探测逻辑本身就是一条要维护的分支，而失败路径（RuleExtractor）本来就有。
- `chat` / `chat_stream` 完全不动（`format=None` 时请求体与现状逐字节一致）。

### 3.3 T2 · 云端腿（`llm/cloud.rs`）

```rust
pub fn chat_body(model, messages, temperature, stream, max_tokens,
                 response_format: Option<&serde_json::Value>) -> serde_json::Value
```

- `chat_structured` 传 `Some(json!({"type":"json_object"}))`；schema 语义由 system prompt 携带（提示词保留「字段含义 + 宁漏勿误」规则行，删「只输出 JSON」的祈祷句——json_object 模式下 DeepSeek 服务端强制纯 JSON）。
- 条目不支持 → `Status` → 兜底。不加能力位、不降级重试。
- `ping` 与普通 `chat` 调用点传 `None`，请求体不变。

### 3.4 T3 · 抽取迁移（`memory/extract.rs`）

```rust
fn extract_via_llm(&self, digest: &SessionDigest) -> Option<Vec<CandidateFact>> {
    let reply = self.client.chat_structured(
        &[Msg::system(extraction_system_prompt()), Msg::user(extraction_user_prompt(digest))],
        &crate::llm::schema::EXTRACTION_FACTS,
        0.2,
        EXTRACT_TIMEOUT_SECS,
    ).ok()?;
    parse_candidates(&reply, digest.user_utterances)   // 原样保留
}
```

- `EXTRACTION_SYSTEM_PROMPT` 删掉「只输出一个 JSON 数组…每条候选的格式：」的格式行，保留语义规则（第三人称、类别定义、白名单、宁漏勿误）。格式描述的唯一权威变成 `EXTRACTION_FACTS`——消灭「prompt 与 Rust 解析两份事实」。
- `strip_fences` 保留（云端模式无害冗余）；`parse_candidates` / `RuleExtractor` 兜底**零改动**。
- 云端模式下 `profile.key` 白名单校验照旧在 Rust（schema 的 enum 压不住动态白名单，这是刻意的分工）。

### 3.5 T4 · 任务分腿路由（`llm/mod.rs` + `llm_admin.rs`）

```rust
/// 两条腿：对话跟随用户主配置；后台任务（夜间作业）默认钉死本地。
pub enum TaskLeg { Chat, Background }

pub fn client_for(leg: TaskLeg) -> Arc<dyn ChatClient> {
    match leg {
        TaskLeg::Chat => current_client(),                      // 现状，零变化
        TaskLeg::Background => background_client(),             // 见下
    }
}

fn background_client() -> Arc<dyn ChatClient> {
    // Manual（现状默认）→ 与 Chat 同源，行为逐字节等价
    // Auto → 恒走本地小模型（llm_admin::override_model() / env / DEFAULT_MODEL）
    if routing_auto() {
        Arc::new(OllamaClient::from_env())
    } else {
        current_client()
    }
}
```

- **路由判定的唯一输入是 `RouteMode`，不是「云端是否配置了」**。Manual 模式下哪怕云端激活，后台也跟主配置走——尊重用户显式选择（他要全云，是他拍的板）；Auto 模式下后台恒本地，设置页文案明示「夜间整理、总结、报告只在你的电脑上跑」。
- `LlmConfig` 增量 `routing: RouteMode`（`RouteMode::{FollowConfig, AutoLocal}`），settings 表新键 `llm_routing_mode`，**`user_version` 不动**（settings 键值先例 §A3）。
- 覆盖层读写、切换免重启语义照旧：`client_for` 每调用点实时读。
- 切换/删除条目的回退语义（`llm/mod.rs:323-328`）原样保留在 `current_client()` 内。

### 3.6 T5 · 后台调用点切腿 + 前端开关

| 调用点 | 现状 | 改后 |
|---|---|---|
| 记忆抽取 `LocalExtractor::new` | `current_client()`（`extract.rs:309`） | `client_for(TaskLeg::Background)` |
| 摘要 L2 重写 `rewrite_digest` 调用方 | 主配置腿（`consolidate.rs:390`） | `client_for(TaskLeg::Background)` |
| 周报/日报 | 主配置腿（`summary.rs:645`、`report.rs:295`） | `client_for(TaskLeg::Background)` |
| 对话 / 工具循环 / persona eval | 主配置腿 | **不动**（`TaskLeg::Chat`） |

前端：设置页模型区新增一块开关（zustand 局部 state + 现有 llm_admin 命令扩展一个字段），文案两行：

- 「后台任务只用本地模型」（默认关）
- 说明行：「开启后，夜间记忆整理、总结与周报只在你的电脑上运行；对话仍按上方选择。」

### 3.7 可观测

- 后台腿调用统一在 `client_for(Background)` 返回处不包（保持纯函数），由各调用点既有的失败日志 + `label()` 覆盖；新增一条 `log::info!("background leg -> {}", client.label())` 在 `background_client()` 内（每次调用打一行，夜间频率可接受）。
- `turn_trace` 不动：路由只影响后台腿，对话轮的 trace 语义不变。

---

## 4. 分期提交（每步可编译 + cargo test/clippy 绿）

| 步 | 内容 | 涉及文件 | 验收 |
|---|---|---|---|
| T1 | `chat_structured` trait 方法（默认 Err）+ `llm/schema.rs` 注册表 + schema/解析互锁单测 | `llm/mod.rs`、`llm/schema.rs`（新） | 桩客户端断言默认 Err；schema 与 `RawCandidate` 互锁测试绿 |
| T2 | Ollama `format` 字段 + `chat_structured` 实现；Cloud `response_format` + 实现；Anthropic 显式不覆写 + 注释说明 | `llm/ollama.rs`、`llm/cloud.rs`、`llm/anthropic.rs` | 请求体单测：无 schema 时字节等价现状；有 schema 时 `format`/`response_format` 字段正确；既有测试零改动全绿 |
| T3 | 抽取迁移 + 提示词去格式化 | `memory/extract.rs` | `parse_candidates` 全部既有测试绿；新增「prompt 不含『只输出 JSON 数组』字样」锁死测试 |
| T4 | `TaskLeg` + `client_for` + `RouteMode` + settings 持久化 | `llm/mod.rs`、`llm_admin.rs` | Manual 模式与现状逐字节等价测试；Auto 模式下 `client_for(Background)` 返回 Ollama、`client_for(Chat)` 返回云的单测 |
| T5 | 三个后台调用点切腿 + 前端开关 + AGENTS.md 澄清句 | `memory/extract.rs`、`memory/consolidate.rs`、`summary.rs`、`report.rs`、`frontend/src/components/settings/*`、`AGENTS.md` | 切腿后 cargo test 绿；前端 tsc/vitest 绿；开关默认关 |

每步独立可编译、独立回滚；T1-T3 与 T4-T5 是两条可并行的线（结构化输出不依赖路由）。

---

## 5. 风险表

| 风险 | 等级 | 缓解 |
|---|---|---|
| sidecar 版本不支持 `format` schema 对象 → 抽取整夜退规则层 | 中 | 失败路径即设计路径（宁漏勿误）；打包前固定 sidecar 版本并真机验收一轮抽取；`llm/serve.rs` 启动健康检查处加版本日志 |
| DeepSeek `json_object` 模式下模型编造字段名 → schema 保证语法但内容漂移 | 低 | 与现状风险面完全相同（提示词纪律下也会编造），Rust 语义校验兜住；schema 反而收窄了编造空间 |
| `chat_body` 签名变更牵连 `ping` / 工具循环 | 低 | 可选参数默认 `None`；T2 验收含「既有请求体字节等价」单测 |
| Auto 模式被误解为「云端也管后台」→ 隐私预期错位 | 中 | 默认关；设置页文案明示两腿边界；opt-in 警告文本核对 spec-m2 §4 承诺 |
| 覆盖层并发读（`client_for` 与 `current_client` 同源）竞态 | 低 | 沿用 `RwLock` 既有模式，无新锁 |
| 退役管线复活嫌疑（结构化输出 ≈ JSON hack？） | — | 本方案是**传输层原生机制**，不是提示词协议 hack；且不触对话管线。AGENTS.md 澄清句（§2.3）把边界写死 |

---

## 6. 决策点（全部闭合）

| # | 问题 | 裁决 |
|---|---|---|
| **D1** | RouteMode 默认 | **FollowConfig**（Manual）；UI 显式开 Auto |
| **D2** | 周报/日报腿 | **Background**（与 §3.6 / 设置页文案一致；Manual 下仍可跟云） |
| **D3** | 本地 7B 第三档 | **V1 不做** |
| **D4** | 将来 JSON 消费点 | 直接 `chat_structured` + schema 注册表 |

---

## 7. 实施记录

| 期 | 门禁 | 说明 |
|---|---|---|
| T1–T5 | `schema_matches` / `default_chat_structured` / `background_leg_pins` / `extraction_prompt_has_no` / ollama+cloud 请求体测；`llm_admin` 13；前端 tsc + ModelSectionRouting 2 | `chat_structured` + `llm/schema.rs`；Ollama `format` / Cloud `json_object`；抽取去格式行；`TaskLeg` + `RouteMode` + `llm_routing_set`；后台调用点切腿；设置页开关；AGENTS.md §模型调用约束 4 |

## 8. 变更记录

| 版本 | 日期 | 说明 |
|---|---|---|
| v0.2 | 2026-11-24 | T1–T5 落地；D1–D4 闭合 |
| v0.1 | 2026-11-24 | 初稿 |

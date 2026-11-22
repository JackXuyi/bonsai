# plan-task-card — 工具调用分流：云端原生 function calling + 本地规则路由

> **状态**：**待评审**（v0.2，2026-11-22）。v0.1 为「任务卡 + 三级管线扩展」；v0.2 按用户裁决重构：**本地模型不做工具调用，工具调用能力全部由云端 API 原生 function calling 承担**（DeepSeek Chat Completion 协议，参考 https://api-docs.deepseek.com/zh-cn/api/create-chat-completion 与 /zh-cn/guides/tool_calls）。原「任务卡」降级为云端循环的审计与 UI 外壳。
> **上游裁决**：2026-11-22 用户指令（本地/云端工具调用分流）；roadmap G-09/G-10（任务模式 + 权限分级，形态随 v0.2 调整）。
> **宪法自检**：云端仅做显式开启的可选增强（2026-11-18 决策不变）；工具参数出设备必须显式披露；人格/记忆上下文不出设备。

## 0. 一句话结论

把「工具怎么被调」从模型侧彻底分流：**云端模式 = DeepSeek 原生 function calling 多轮循环（模型只决定调什么，Rust 执行 + 结果回传）；本地模式 = 零模型工具调用，只保留代码层规则路由兜底**。原三级渐进 JSON 管线（为 2B 模型手搓的协议）在云端路径退役，本地路径保留直答。

## 1. 现状锚点（设计依据，全部实测）

| 现状 | 位置 | 对方案的约束 |
|---|---|---|
| `ChatClient` trait **无工具调用能力**：`chat(&[Msg])` 返回 `String`，`Msg` 只有 system/user/assistant 三角色 | `llm/mod.rs:74`、`Msg`（104 行） | 工具调用是**新增 trait 能力**，不是改参数——加默认实现（`Err(unsupported)`）即可让三实现零改动 |
| 云端已有两个实现：`CloudClient`（OpenAI 兼容 `/chat/completions`）+ `AnthropicClient`（原生 `/v1/messages`）；条目缺失回落本地 | `llm/cloud.rs` / `anthropic.rs` / `choose()` | DeepSeek 走 OpenAI 兼容协议，`CloudClient` 是现成承载；协议差异只收在工具方法内部 |
| 三级渐进 JSON 管线（level0 目录 → 候选 → 决策 → pass-3 摘要）是**为 2B 不可靠 function calling 手搓的协议** | `dialogue.rs::agent_pipeline` | 云端路径用原生协议整体替代；本地路径退役该管线（§5） |
| 人格只在面向用户轮注入、协议轮剥离（2026-11-21 裁决，2B 稀释 JSON 的教训） | `dialogue.rs` | 该裁决是 2B 专属；DeepSeek 上下文宽裕，人格可注入循环全程 system prompt（§7，D6 裁决） |
| `execute_audited`：validate_args → ApprovalGate → 执行 → 审计（脱敏/锁收敛已就绪） | `mcp_admin.rs` | 循环的每一步就是一次 `execute_audited`，零改动复用 |
| 日历规则路由：纯代码层，不经过模型 | `tools.rs`（20 正/15 负 CI） | 本地模式的工具兜底（§5）；死模型也答得了日程 |
| `memory_search` recall：内置工具 + 目录置顶 + 强指令（模型自选候选） | `agent_tools.rs::builtin_tools` / `dialogue.rs` | 本地模式改为代码层 recall 路由（§5）；云端模式进 tools 列表 |
| schema v11 | `memory/db.rs:28` | 任务卡外壳表用 **v12**（§8） |
| 审批：AutoAllowGate 默认放行，AskFirstGate 已实现未真用 | `agent_tools.rs` | 权限三档（§6）首次真用 ask 档 |
| **Chat Completion 不支持对话中间插入工具调用**（Anthropic/Responses API 才支持） | DeepSeek tool_calls 指南 | 工具轮消息必须是尾部连续结构：`user → assistant(tool_calls) → tool → … → assistant(content)`；bonsai 现有「工具结果塞 system」的拼法在新管线中不得混用 |
| 思考模式下 `tool_choice` 不支持 `required`/指定工具（400）；`deepseek-flash` 非思考模式 temperature 恒 1.0 | DeepSeek 主文档 | 工具循环统一 `thinking: disabled` + `tool_choice: auto`，不踩协议坑 |

### 1.1 DeepSeek 协议要点（映射 bonsai 概念）

| DeepSeek 协议 | bonsai 侧 |
|---|---|
| 请求 `tools: [{type:"function", function:{name, description, parameters}}]` | 由 `ToolDescriptor` 生成：`name`→name、`detail`→description、参数 schema→parameters（MCP inputSchema / 内置工具声明） |
| `tool_choice: none/auto/required` | 恒 `auto`（kit 为空时不传 `tools`——协议规定无 tools 时默认 none） |
| 响应 `finish_reason="tool_calls"` + `message.tool_calls[]{id, function.name, function.arguments(JSON 字符串)}` | 循环继续：白名单校验 name → validate_args → ApprovalGate → `execute_audited` |
| 回传 `{"role":"tool","tool_call_id":<id>,"content":<结果>}` | 回传 `ToolOutput.summary`（≤200 字），**full 永不出库** |
| 一次响应可含多个 tool_calls（并行） | 顺序执行（写入序依赖审计）；全部回传后再请求 |
| `strict` 模式（Beta 端点）：输出严格符合 JSON Schema，但要求全属性 required + `additionalProperties:false` | 默认不启用（D5）：MCP schema 多不满足 strict 约束，`validate_args` 本地校验兜底 |

## 2. 总体架构

```
对话管线（dialogue.rs）
  危机拦截（不动）
    │
    ├─ 云端模式（kit 非空 && 当前 client 支持 function calling）
    │    ① system prompt = 环境段 + L1 宪法 + skill 段 + L2/L3（人格全程在，D6）
    │       tools = ToolDescriptor 生成的 function 列表（tool_choice=auto）
    │    ② 请求 → finish_reason=tool_calls → 逐个：白名单/validate_args/审批/execute_audited
    │       → assistant(tool_calls) + tool 结果消息追加 → 回到 ②（轮次上限 §4）
    │    ③ finish_reason=stop → content 即最终回复（人格口吻，云端模型直接产出）
    │
    ├─ 本地模式（或云端不可用/不支持工具）
    │    ① 代码层规则路由：日历意图（tools.rs）+ 记忆召回（recall 触发词，§5）
    │    ② 命中 → Rust 执行 → 摘要进 prompt → 本地模型只做措辞（不改写事实）
    │    ③ 未命中 → 纯对话直答（无 tools 概念，三级 JSON 管线退役）
    │
    └─ 降级链：云端请求失败/超时 → 该轮按本地模式处理（显式提示，不静默）
```

**分流原则（用户裁决落地）**：本地模型**永远不产生**工具调用决策——没有 candidates JSON、没有 memory_search 强指令；「该不该用工具」要么云端模型答，要么代码规则答。

## 3. ChatClient 扩展设计

```rust
// llm/mod.rs 新增（不动既有 chat/chat_stream —— 单测与 consolidate 零回归）
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ToolSpec { pub name: String, pub description: String, pub parameters: serde_json::Value }

#[derive(Debug, Clone)]
pub struct ToolCallReq { pub id: String, pub name: String, pub arguments: serde_json::Value }

pub struct ToolReply {
    pub content: Option<String>,          // finish_reason=stop 时的正文
    pub tool_calls: Vec<ToolCallReq>,     // finish_reason=tool_calls 时非空
    pub finish_reason: String,
}

pub trait ChatClient: Send + Sync {
    // …既有三方法不动…

    /// 工具调用轮。不支持工具的实现（Ollama）返回 Err——
    /// 「本地模型不执行工具调用」由类型系统强制，而非运行时约定。
    fn chat_with_tools(&self, _messages: &[ToolMsg], _tools: &[ToolSpec],
                       _temperature: f64, _timeout: u64) -> Result<ToolReply, LlmError> {
        Err(LlmError::Decode("tool_unsupported".into()))
    }
}
```

- `ToolMsg`：`Msg` 的工具扩展（role 增 `tool`；assistant 消息可带 `tool_calls`；tool 消息带 `tool_call_id`）——**单独结构**，不复用 `Msg`，避免污染全部既有调用点。
- `CloudClient::chat_with_tools`：按 DeepSeek/OpenAI 兼容协议实现（§1.1 映射表）。`AnthropicClient` 暂不实现（默认 Err → Claude 条目自动走本地路径，不阻塞主线，D7）。
- SSE 流式：工具循环的中间轮**非流式**（响应是 tool_calls JSON）；只有 `finish_reason=stop` 的最终轮可流式。v1 先非流式全程，流式作后置优化（与 agent-mcp-skill「流式只在第 3 轮」同构）。

## 4. 云端工具循环（护栏）

| 护栏 | 值 | 理由 |
|---|---|---|
| 单轮对话工具调用总次数 | ≤8（每响应可含多个 tool_calls） | 防失控循环；超出 → 强制收尾：把已得摘要链注入并要求直接作答 |
| 连续 HTTP 请求次数 | ≤6 | DeepSeek 延迟秒级，网络往返比本地推理贵，上限要更紧 |
| 每步执行 | 逐个串行 `execute_audited` | 审计写入序 + 审批卡逐个出（§6） |
| 参数校验 | 白名单 → `validate_args` → 审批 | 协议明说「模型可能臆造未定义参数」——本地校验是硬兜底 |
| 工具结果回传 | 仅 `ToolOutput.summary`（≤200 字），`full` 永不出库 | 外部内容 untrusted 纪律不变（G-12） |
| 单步失败 | 不重试；失败以 `role=tool` 回传错误文本让模型自行决策 | 云端场景重试的价值低于把失败事实告诉模型 |
| 失败消息 | `tool` 消息 content = 错误摘要（脱敏后） | 模型可以说「查不到」，不许编造 |
| 循环中断 | 用户「别做了」→ 下一响应前终止，模板收尾 | 沿 v0.1 取消语义 |
| 隐私 | 进 tools 的只有工具名+描述+参数 schema；**记忆/人格正文不出设备**；审计 `remote=1`；UI 添加云端条目时明示「工具参数会发往 <base_url>」 | 隐私定位红线；沿 plan-agent-mcp-skill 远程 server 同款披露 |

## 5. 本地路径收敛（本版动存量行为最多的一块）

| 存量 | 处置 |
|---|---|
| 三级渐进 JSON 管线（agent_pipeline 的 level0/1/2 + pass-3） | 本地模式退役：删除模型候选/决策轮；`agent_tools.rs` 解析器（parse_candidates 等）保留供单测与未来复用 |
| `memory_search` recall 置顶 + 强指令 | 本地模式改为**代码层 recall 路由**：时间词/「聊过/说过/记录」触发词命中 → 直接执行 memory_search → 摘要进 prompt 让本地模型措辞（与日历规则路由同构，20 正/15 负 CI 惯例） |
| 日历规则路由 | **不动**（本来就是零模型工具调用） |
| 云端模式下的三级管线 | 整体不进云端路径（native function calling 替代） |
| `tool_calls` 审计 | 两条路径同一张表，kind 区分来源；本地 recall 路由照常落审计 |

**本地模式的能力边界要摆在明面上**：设置页工具卡注明「本地模式下仅支持日程与记忆召回；完整工具调用需启用云端模型」。不是隐藏，是诚实。

## 6. 权限三档（沿 v0.1，接线点改为云端循环）

| 档 | 判定 | 行为 |
|---|---|---|
| `read` | 工具声明 `access: read`（默认） | 直接执行，审计 `auto_allowed` |
| `ask` | 工具声明 `access: write` | 强制 AskFirstGate（无视 `approval_mode`）；确认卡逐个出，不许一揽子授权 |
| `forbidden` | 删除/外发类 | 引擎直接拒，回传「该操作被禁止」让模型告知用户 |

`ToolDescriptor` 增 `access` 字段（默认 read，向后兼容）。审批钩子位置不变（validate_args 之后、执行之前）。

## 7. 人格边界（v0.2 修订）

1. **云端模式人格全程在线**（D6 待裁决）：system prompt 含环境段 + L1 + skill 段 + L2/L3——2B「人格稀释 JSON」的教训不适用于 DeepSeek；循环自然结束时模型直接以阿盆口吻作答，**pass-C 润色轮退役**（省一次请求）。
2. **本地模式人格照旧**：单轮直答 + 规则路由措辞轮，注入方式不变。
3. **工具事实不被改写**（沿日历教训的云端版）：`role=tool` 回传的是事实原文摘要，模型只做表达层组织；关键事实（数字/时间/条数）以审计表为准可查。若实测发现编造事实，回退「模板答案 + 模型只许接一句话」（D3 裁决）。
4. **skill/MCP 内容 untrusted 纪律不变**：不进记忆抽取，不入事实库。
5. **关系档照常生效**：云端回复同走 prompt 四档；dialogue 事件照常喂成长线——干活与养成不分家。

## 8. 任务卡外壳（v0.1 的降级保留）

云端循环天然就是「多步执行」，独立任务卡实体收缩为**审计与 UI 外壳**：

- schema v12 `tasks`：一次云端循环（产生 ≥1 次工具调用的对话轮）落一行——goal 取用户原话前 80 字、status（done/failed/canceled）、工具调用数；步骤明细仍在 `tool_calls`（增 task_id 关联列）。**不再让 LLM 预填步骤 JSON**——循环里的每步就是真实步骤。
- 前端任务卡：循环期间实时展示「第 N 步 · 工具名 · 状态」（Channel 事件沿 v0.1），ask 档确认卡嵌入卡内。
- `tasks` 审计不进 ACTIVITY_KINDS（自己陪自己防线的延伸）。

## 9. 里程碑

| # | 事项 | 交付 | 依赖 |
|---|---|---|---|
| T1 | 协议层 | `ToolMsg`/`ToolSpec`/`ToolReply` + `chat_with_tools` 默认 Err + `CloudClient` DeepSeek 实现（Mock 单测，请求/响应形状对齐官方文档样例） | 无 |
| T2 | 云端循环引擎 | dialogue.rs 云端分支：tools 生成 / 循环 / 上限护栏 / 失败回传 / 轮次事件；`execute_audited` 接线 | T1 |
| T3 | 权限三档 | `ToolDescriptor.access` + ask 档强制 AskFirstGate + forbidden 拒绝 + 确认卡事件 | T2 |
| T4 | 本地路径收敛 | 三级管线退役 + recall 代码路由（CI 惯例）+ 设置页能力边界说明 | T2（可并行） |
| T5 | 任务卡外壳 + UI | schema v12 tasks + 循环进度卡 + 确认卡组件 + 🔧 标注（kind 前缀沿现状） | T3+T4 |
| T6 | 真机验收 + 文档 | DeepSeek 真连：工具命中率/误触发率、人格口吻回归样本、断网降级链、审批覆盖率 100% 核验、token 成本记录 | T5 |

每里程碑 cargo check + 全量单测 + `pnpm app` 真机过一遍再进下一个。

## 10. 风险

| 风险 | 档 | 对策 |
|---|---|---|
| 本地模式能力缩水（recall 从「模型自选」退到触发词，召回变钝） | **高** | 触发词覆盖主流问法 + CI 30 样本；云端模式 recall 走原生 function calling 更强；实测漏召回高则本地补「每轮必查 recent N 条摘要」的廉价兜底 |
| 云端 API 费用失控（工具循环放大 token） | 中 | tools 描述紧凑（detail 单行化）；循环上限；`usage` 落审计；设置页显示当日用量 |
| 隐私稀释（工具参数出设备） | **高** | §4 隐私行：只出参数、不出记忆/人格；添加条目强制披露；审计 remote 标记；本地模式永久不出设备 |
| DeepSeek 协议漂移（非 OpenAI 完全兼容的扩展字段） | 中 | 只用标准 tools/tool_calls/tool 消息三件套（官方文档明确支持）；DeepSeek 扩展字段（title/safety_check/server_tool_calls）不碰 |
| 思考模式与 tool_choice 冲突（400） | 低 | 循环统一 `thinking: disabled`；单测锁定请求体形状 |
| 中间步失败后模型胡编「成功」 | 中 | 失败以 tool 消息显式回传 + 任务卡标 failed；T6 样本回归 |
| `finish_reason=tool_calls` 后模型不收尾（请求死循环） | 中 | 请求上限强制收尾轮 + 已得摘要链注入 |
| 人格漂移（云端模型被 skill 带偏） | 中 | L1 优先级声明 + 人格回归样本（沿 S7 惯例） |

## 11. 待裁决决策点

| # | 问题 | 建议 | 备选 |
|---|---|---|---|
| D1 | 本地模式工具覆盖 | 日历 + 代码层 recall 路由，其余全部云端 | 本地零工具（连 recall 都不做） |
| D2 | 循环上限 | 工具调用 ≤8 / HTTP 请求 ≤6 | 更紧（4/3）省钱；更松（12/9）保长任务 |
| D3 | 云端最终回复对工具事实的处理 | 允许模型表达层润色（审计表为准） | 模板答案 + 模型只接一句话（2B 时代教训的保守版） |
| D4 | 权限三档默认 | read 直行 / write 强制 ask / forbidden 拒 | 跟随全局 approval_mode |
| D5 | DeepSeek strict 模式 | 不启用（MCP schema 多不满足约束，validate_args 兜底） | 对内置工具单独启用（Beta 端点） |
| D6 | 云端循环 system prompt 是否含人格全程 | **含**（省 pass-C，DeepSeek 上下文宽裕） | 只在最终轮注入人格（多一次请求，事实零改写最稳） |
| D7 | AnthropicClient 工具支持 | 本版不做（默认 Err 自动走本地路径） | 同期实现 Claude tool_use（T1 工作量约翻倍） |

## 12. 实施记录（2026-11-22，D1–D7 全按推荐落地）

| 里程碑 | commit | 内容 | 验证 |
|---|---|---|---|
| T1 协议层 | a545b93 | `ToolSpec`/`ToolCallOut` 手写 Serialize（`{type:"function",function:{...}}` 嵌套）+ `ToolMsg` 五种构造器 + `chat_with_tools` ChatClient 默认 Err（本地不做工具调用由 trait 形状强制）+ CloudClient 非流式实现 + `tool_chat_body` 空 tools 整体省略键 | cargo test |
| T3 权限三档 | 485c776 | `ToolAccess`（read/ask/forbidden，serde lowercase）+ `execute_audited` 三档分流 + ask 档 `ask_gate` 强制 AskFirstGate + forbidden 拒绝并审计 `forbidden_tool` + 审计增 task_id | cargo test |
| T2+T4 分流收敛 | 1669b66 | dialogue.rs 删 agent_pipeline/prose_retry/persona_answer（三级 JSON 管线退役）+ `cloud_tool_loop`（cap 8/6、强制收尾轮、失败脱敏回传、惰性 task 卡）+ `local_recall_answer` 代码层强制 recall 路由 + skill 纯提示注入（untrusted+预算≤2） | cargo test |
| T5 后端 | 40f823f | schema v12：`tasks` 表（TEXT uuid id）+ `tool_calls.task_id` + `task_start/task_finish/task_list` + 循环经 Channel 发 `{"task_step":...}` 事件 | cargo test |
| T5 前端 | 7fc4479 | `TaskInfo/TaskStep` 类型 + useChat `applyTaskEvent`（task_start 补建/去重/终态对齐）+ MessageBubble 任务卡气泡 + chat.css | tsc + vitest 91 过 |

**最终状态**：后端 `cargo test --lib` 335 全绿；前端 `tsc --noEmit` 绿 + vitest 27 文件 91 测试全过。

**环境备注**：`next build` 在 WorkBuddy 宿主 shell 内因 fs shim（brokered-fs / safe-delete 拦截 `.next` 读写删除）无法完成，属宿主环境限制而非代码问题；生产构建请在用户自有终端执行 `npm run build` 验证。

**T6 真机待验项**（需 DeepSeek 真连 + GUI 交互，代码层无法自验）：
- DeepSeek 真连工具命中率 / 误触发率
- 人格口吻回归样本（云端循环收尾轮的阿盆语气）
- 断网降级链（云端失败回落本地路径）
- ask 档确认卡 GUI 流程（弹窗放行/拒绝）
- token 成本记录（usage 落审计的实际读数）

**遗留裁决兑现说明**：D7 未做 Claude tool_use——AnthropicClient 的 `chat_with_tools` 走默认 Err，工具请求自动回落本地路径。

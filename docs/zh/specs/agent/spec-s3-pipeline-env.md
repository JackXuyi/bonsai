# spec-s3 — 三级渐进管线 + 环境段

> 依赖：S2。目标：三级渐进工具选择接进 chat_turn；system prompt 注入环境段。默认 prompt 只带名称，逐轮加码（plan §3.3）。

## 1. 统一抽象（agent_tools.rs）

```rust
pub struct ToolDescriptor {
    pub name: String,       // 目录级（第 0 级）
    pub summary: String,    // 筛选级（第 1 轮，≤40 字）
    pub detail: String,     // 完整级（第 2 轮）：inputSchema 概要 / skill 正文
    pub kind: ToolKind,     // Mcp | Skill
    pub remote: bool,
}
pub struct ToolCall { pub name: String, pub args: serde_json::Value }
pub struct ToolOutput { pub summary: String, pub full: String }   // summary ≤200 字
pub trait ToolProvider {
    fn descriptors(&self) -> Vec<ToolDescriptor>;
    fn execute(&self, call: &ToolCall) -> Result<ToolOutput, String>;
}
```

## 2. 环境段（persona/prompt.rs 新增 `env_segment`）

```
【环境】当前时间：2026-11-20 11:45（周五）· 时区 Asia/Shanghai (UTC+8) · 语言：简体中文 · macOS
```

- 签名 `env_segment(now: DateTime<Local>, language: &str) -> String`——**时间由调用方传入**（clock 注入纪律，防真实时钟测试 bug 复发）；语言取 settings `language`（默认「简体中文」）；平台 `std::env::consts::OS`。
- 注入位置：`dialogue::system_prompt` 里，`render()` 输出**之前**（L1 前），每轮必在，≤120 字符。

## 3. 三级管线（dialogue.rs 扩展 `chat_turn_inner`）

激活条件：**存在启用的 ToolProvider**（MCP 启用 server 或启用 skill）。无 provider 时管线零介入——现有全部测试路径不变。

```
第 0 级：system prompt 追加
  【可用能力（仅名称）】mcp_tool_a · mcp_tool_b · skill_x（≤20 项）
  指令：需要用能力时只输出 {"candidates": ["name", …]}；否则正常回答。
第 1 轮（模型第 1 次调用，不流式）：
  → 解析 candidates（严格 JSON；解析失败/为空/超 5 个 → 本轮回复即最终回复，管线结束）
  → 白名单校验：候选必须命中 descriptors（未知名丢弃）
  → 无剩余候选 → 同上结束
第 2 轮（模型第 2 次调用）：
  system prompt 追加候选的 summary + detail（skill 正文 ≤2 个、单 skill ≤8KB）
  → 解析 {"tool": name, "args": {...}}：
     - 解析失败（模型直接用 skill 作答或放弃）→ 本轮回复即最终回复，结束
     - tool 校验：命中白名单 + args 是 JSON object → 审批钩子（S4）→ execute（60s）
       失败 → Err → 记审计，正常对话收尾（不把错误文本丢给模型）
第 3 轮（模型第 3 次调用，仅工具路径）：
  注入「工具结果摘要（≤200 字）」→ 模型在摘要上接一句话；流式在此轮开启（on_delta）
```

- **流式降级（偏差记录）**：spec-m2 的流式在三级管线激活时退化为「最终回复一次性发一个 delta」——第 1 轮不能流式（输出可能是候选 JSON），MVP 接受；无 provider 时保持逐字流式不变。
- **静默降级**：任何一轮不合法 → 正常对话收尾，不重试、不提示报错。
- **untrusted**：工具输出/skill 正文不写 events（抽取面天然干净）；consolidate 无需新拦截（事件层本来就不收）。
- DialogueOutcome.tool 扩展：`mcp:<tool_name>` / `skill:<skill_name>`，前端据此前缀标注。

## 4. 测试清单（CI，~10 测）

1. env_segment：固定 now → 时间/星期/时区正确；语言可替换（2）→ `env_prefix_carries_time_and_language`
2. candidates 解析：合法 JSON/非法/超 5 个截断（3）→ `parse_candidates_handles_wrapped_json_and_garbage`
3. 白名单：未知名丢弃、全丢 → None（1）→ `pipeline_unknown_candidate_dropped_and_denied_call_templates`
4. Mock 三级全链路：pass1 出候选 → pass2 出 tool → pass3 摘要作答；tool_calls 落库（1）→ `pipeline_three_rounds_full_chain_is_audited`
5. skill 路径：pass2 直接作答（无执行），kind=skill 落库（1）→ `pipeline_skill_body_answers_without_execution`
6. 无 provider → 走原路径、流式逐字不变（1）→ `chat_turn_streaming_deltas_and_records_match_chat_turn`、`capability_section_appears_only_when_tools_exist`
7. 静默降级：pass1 出垃圾 → 正常回复、无审计行（1）→ `pipeline_degrades_silently_on_garbage`
8. tool 超 20 截断按字典序稳定（1）→ `name_catalog_lists_up_to_20`、`catalog_is_sorted_and_capped`
9. 第 0 级只注入名称（不含 summary）：`capability_section_appears_only_when_tools_exist` 断言 prompt 含 `weather` 但不含 `查天气`

## 5. 验收清单

- [x] 上述 CI 测全绿；现有 dialogue 测试零改动通过
- [x] 无 provider 时**不追加任何能力段**（`capability_section_appears_only_when_tools_exist`）
      —— 注：环境段（D6）是无条件注入的，所以「与 v7 逐字节一致」的说法不成立，改为「能力段零介入」这个更准的断言。
- [ ] 真机：三级管线轮次成本实测（无候选 1 轮 / 全链路 3 轮），延迟可接受
- [ ] 真机：2B 模型对「今天有什么会」仍走日历规则路由（管线不抢既有意图）
- [ ] 真机：环境段注入后时间类问答（「今天几号」）不再需要规则词表兜底

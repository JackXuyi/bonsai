# spec-s4 — 审批钩子 + 审计

> 依赖：S3。目标：`ApprovalGate` trait + 默认 AutoAllowGate（用户拍板 D10：当前放行）；调用全链路审计落库，UI 可查（UI 在 S6）。

## 1. ApprovalGate（agent_tools.rs）

```rust
pub enum ApprovalDecision { Allow, Deny }
pub trait ApprovalGate: Send + Sync {
    fn check(&self, call: &ToolCall, source: &str, remote: bool) -> ApprovalDecision;
}
pub struct AutoAllowGate;                 // 当前默认：放行，审计记 auto_allowed
pub struct AskFirstGate { /* emit 事件 + 等待结果的通道骨架 */ }  // S6 接 UI 后启用
```

- 钩子位置：参数校验**之后**、执行**之前**（校验不过不进审批）。
- settings `approval_mode`：`auto`（默认）| `ask`。切 ask 即换 trait 对象，调用方零改动。
- AskFirstGate 实现（S4+S6 已接线，不再是骨架）：
  `ApprovalBroker` 单槽握手（`register` → emit `tool:approval_required {id, tool, source, remote, argsSummary}` → 前端 `mcp_approval_reply(id, allow)` → `Condvar` 唤醒）。
  超时 30s（`APPROVAL_WAIT`）或**无 AppHandle（headless/单测）** ⇒ Deny；回执带不匹配 id ⇒ 忽略，不消耗槽位。
  命令 `mcp_approval_reply` **不碰 DB**：对话轮次等审批时持有 store 互斥锁，回执路径必须与数据库解耦才不会自锁。
  前端卡片收到 `false`（已超时/已答复）→ 显示「已过期」，不再重复回执。

## 2. 审计（memory/db.rs 扩展）

```sql
-- tool_calls 新列（S2 已迁）：params_summary / result_status / prompt_trace / remote
```

- 每次工具调用（含 skill 使用）写一行：tool=`mcp`|`skill`、action=`<descriptor name>`、outcome=`ok|denied|unavailable|error`、result_status 补充细类（`mcp_timeout`/`mcp_unreachable`/`auto_allowed`…）、params_summary 脱敏 ≤200 字、duration_ms、remote 0/1。
- **prompt_trace**：三级管线各轮注入片段的引用（轮次号 + 片段 hash + 候选名单 + 最终选择），存 bonsai.log（debug 级），审计表存引用串（≤200 字）。出问题可复现「模型当时看到了什么」。
- 明文纪律：params_summary/trace 脱敏（headers/token 不入审计）。
- 读取命令：`tool_call_records(limit)` → 最近 N 行倒序（供 S6 调用记录卡）。

## 3. 测试清单（CI，~6 测）

1. AutoAllowGate：check 恒 Allow（1）→ `auto_gate_allows_ask_gate_denies`
2. 审计落库：三级全链路后 tool_calls 行数/字段正确（1）→ `pipeline_three_rounds_full_chain_is_audited`、`execute_audited_records_ok_and_failure_status`
3. denied 路径：gate=Deny → 不执行、outcome=denied、对话正常收尾（1）→ `pipeline_unknown_candidate_dropped_and_denied_call_templates`
4. 失败路径：execute Err → outcome=error、result_status=mcp_timeout（1）→ `execute_audited_records_ok_and_failure_status`
5. params_summary 脱敏：含 `Bearer x` 的参数 → `••••`（1）→ `execute_audited_records_ok_and_failure_status`、`audit_rows_round_trip_masked`
6. tool_call_records：倒序 + limit（1）→ `tool_call_records_newest_first_with_limit`
7. 审批握手：无 UI 直接 Deny、id 不匹配不消耗槽位、重复回执无效（1）→ `approval_broker_handshake`

## 4. 验收清单

- [x] 上述 CI 测全绿
- [x] grep 确认审计写入路径无明文 token
- [ ] 真机：approval_mode=ask 时审批卡事件到达前端、30s 超时拒绝生效
- [ ] 真机：调用记录卡与 tool_calls 实况一致

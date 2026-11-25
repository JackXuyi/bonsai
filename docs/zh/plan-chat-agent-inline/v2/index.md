# 方案：本会话批量授权 + 工具参数可读化（plan-chat-agent-inline v2）

> **状态**：已实施（2026-11-25）  
> **承接**：[v1](../v1/index.md)（Q2「本轮/本会话此工具」本期落地为**本会话**）  
> **一句话**：同会话重复调用同一工具可一次授权；审批/审计/追踪里的参数按键值分行展示。

---

## 1. 批量授权

| 项 | 规格 |
|---|---|
| UI | 授权条增加 **「本会话允许」**（保留「允许本次」「拒绝」） |
| 回执 | `mcp_approval_reply(id, allow, scope)`；`scope=session` 时写入白名单 |
| 存储 | `AgentState.session_allow`（内存，不落库）；`bind_session` 换会话清空 |
| 门控 | `AskFirstGate` 白名单命中 → 直接 Allow，不弹卡 |
| 仍拦 | `ToolAccess::Forbidden`；其它工具名；换会话后须重批 |

不实现：一揽子批准整份计划所有写操作；永久/跨会话记住。

## 2. 参数可读化

| 层 | 行为 |
|---|---|
| Rust `format_args_summary` | 脱敏后按工具分行（`fs_write` 路径+内容预览；`fs_copy/move` 从/到；通用 `键：值`） |
| 前端 `ToolArgsView` | 授权条 / 审计展开 / 追踪展开共用键值列表 |

## 3. 权威实现

- `src-tauri/src/mcp_admin.rs` — `SessionToolAllow` / broker `peek_tool` / reply `scope`
- `src-tauri/src/agent_tools.rs` — `format_args_summary`
- `frontend/src/components/agent/InlineApprovalBar.tsx`、`ToolArgsView.tsx`
- `frontend/src/lib/formatToolArgs.ts`

## 4. 验收

- [ ] 同会话连续两次 `fs_write`：第一次点「本会话允许」后第二次不弹卡
- [ ] 换会话后再调同工具会重新询问
- [ ] 授权条参数可见「路径：」等分行，而非单行挤 JSON

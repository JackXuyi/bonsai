# 方案：拆分 `mcp_admin` 巨石（AGENTS §12）

| 文档信息 | 内容 |
|---|---|
| 版本 | v1 |
| 日期 | 2026-11-25 |
| 状态 | **已实施** |
| 上游 | 优化盘点 P1；[`plan-dialogue-split`](../../plan-dialogue-split.md) 范式；AGENTS §3/§6/§12 |
| 一句话 | **只搬家不改脑子**：`mcp_admin.rs` → `mcp_admin/` 按域拆文件，公开类型路径稳定，Tauri 命令走公开子模块 |

---

## 布局

| 文件 | 职责 | 行数（约） |
|---|---|---|
| `mod.rs` | 常量、`now_conn`、再导出 | ≤100 |
| `state.rs` | `AgentState` / `SessionToolAllow` | ≤100 |
| `servers.rs` | MCP SQL + 服务器命令 | ≤700 |
| `approval.rs` | `ApprovalBroker` / `AskFirstGate` | ≤700 |
| `runtime.rs` | kit / `execute_audited` / cancel | ≤700 |
| `skill_cmds.rs` | skill_* 命令 | ≤700 |
| `gate_cmds.rs` | 审计 / plan / task / budget | ≤700 |
| `tests.rs` | 原 colocated 单测 | ≤700 |

`lib.rs` `generate_handler!` 指向 `mcp_admin::{servers,runtime,skill_cmds,gate_cmds}::*`（对齐 `dialogue::commands`）。

## 非目标

- 不改审批 / 审计 / 挂载语义；不引入新抽象层；不拆 consolidate 写阶段（见 plan-store-lock D4）。

## 变更记录

| 版本 | 日期 | 说明 |
|---|---|---|
| v1 | 2026-11-25 | 落地拆分；单测 `mcp_admin` / `mcp_discover` 绿 |

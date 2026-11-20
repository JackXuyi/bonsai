# spec-s1 — MCP 客户端核心（rmcp）

> 依赖：无。目标：`mcp.rs` 提供统一 `ToolProvider` 门面下的 stdio + Streamable HTTP 双传输 MCP 客户端。三级管线/审批/审计不感知传输差异。

## 1. 依赖接入（Cargo.toml）

```toml
tokio = { version = "1", features = ["rt-multi-thread", "macros", "process", "time"] }
rmcp = { version = "<registry 最新稳定>", features = [
  "client", "transport-child-process", "transport-streamable-http", "transport-sse",
] }
```

- tokio 运行时：Tauri 2 自带，`tauri::async_runtime::block_on` 做同步门面，**不另起 runtime**。
- async 边界收在 mcp.rs 内；`lib.rs` 其余模块保持同步风格。

## 2. 模块设计（src-tauri/src/mcp.rs）

```rust
/// 单个 MCP server 的连接。生命周期由 McpManager 管理，不是 per-call。
pub enum McpTransport {
    Stdio { command: String, args: Vec<String>, env: Vec<(String, String)> },
    Http { url: String, headers: Vec<(String, String)> },
}

pub struct McpClient { /* rmcp RunningService + server_id */ }

impl McpClient {
    pub fn connect(t: &McpTransport) -> Result<Self, String>;   // block_on(serve_client)
    pub fn list_tools(&self) -> Result<Vec<ToolDescriptor>, String>;  // 60s 超时
    pub fn call_tool(&self, name: &str, args: serde_json::Value) -> Result<ToolOutput, String>;
    pub fn ping(&self) -> bool;                                  // 健康检查
    pub fn shutdown(self);                                       // 显式 kill，防孤儿
}
```

- **超时**：连接 10s、list/call 60s（tokio `time::timeout`）。超时 → `Err("mcp_timeout")`。
- **不可达**：连接失败 → `Err("mcp_unreachable")`；绝不用空结果冒充成功（日历教训）。
- **孤儿回收**：`McpManager::shutdown_all()` 在 `RunEvent::Exit` 调用（沿 sidecar 的 stop 模式）；进程 spawn 用 kill_on_drop。
- **Mock 优先**：`ToolProvider` trait（agent_tools.rs）下，`MockMcpProvider` 供单测；真 rmcp 路径只在真机验收跑（.app 环境子进程行为 CI 无法覆盖）。

## 3. 测试清单（CI，~6 测）

1. transport 解析：server 行 → McpTransport（stdio/http 各 1）→ `transport_from_row_validates_fields`
2. MockMcpProvider：descriptors 稳定排序 + execute 返回摘要（2）→ `mock_provider_executes_and_fails`、`catalog_is_sorted_and_capped`（排序在 `AgentKit::catalog`，provider 层保持给定顺序）
3. 工具结果截断：>200 字摘要截断 + full 保留全文（1）→ `mock_provider_truncates_summary_but_keeps_full_text`
4. 超时错误码：mock 卡死 → `mcp_timeout`（1）→ `mock_provider_executes_and_fails`（fail_for 路径）
5. unreachable 错误码：假端口 → `mcp_unreachable`（1）→ `unknown_tool_is_unreachable_not_empty_success` + `toggle_flow_with_mock_client`（真连接失败）

> 偏差：真 rmcp 的 timeout/unreachable 用活连接打假端口才能覆盖，CI 只验错误码出口；超时/断网的**实际**行为走 S7 真机项。

## 4. 验收清单

- [x] cargo check 干净，rmcp/tokio 依赖编译通过
- [x] 上述 CI 测全绿
- [ ] 真机：本地 stdio MCP server（如 filesystem）list_tools 成功、tool_cache 落库
- [ ] 真机：远程 HTTP MCP server 连接 + list_tools + call_tool 全链路
- [ ] 真机：断网/杀进程 → 对话收到显式报错，非空结果、非挂死
- [ ] 真机：应用退出无孤儿 MCP 子进程（`pgrep -f <server 命令>`）

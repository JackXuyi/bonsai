# spec-s2 — schema v8 + server 管理

> 依赖：S1。目标：v7→v8 幂等迁移 + `mcp_admin.rs` CRUD 命令 + 设置页接线（UI 在 S6）。

## 1. schema v8（memory/db.rs）

- `SCHEMA_VERSION = 8`；migrate 追加（幂等，沿 add_column_if_missing 惯例）：

```sql
CREATE TABLE IF NOT EXISTS mcp_servers (
  id            TEXT PRIMARY KEY,
  name          TEXT NOT NULL,
  transport     TEXT NOT NULL DEFAULT 'stdio',  -- 'stdio' | 'http'
  command       TEXT,                            -- stdio 必填，http 为 NULL
  args_json     TEXT NOT NULL DEFAULT '[]',
  url           TEXT,                            -- http 必填
  headers_json  TEXT NOT NULL DEFAULT '{}',      -- 可含 Authorization；读取路径脱敏
  env_json      TEXT NOT NULL DEFAULT '{}',
  enabled       INTEGER NOT NULL DEFAULT 0,      -- 新增一律 disabled（RK-04）
  tool_cache    TEXT,
  tool_cache_at TEXT,
  created_at    TEXT NOT NULL
);
ALTER TABLE tool_calls ADD COLUMN params_summary TEXT;
ALTER TABLE tool_calls ADD COLUMN result_status  TEXT;
ALTER TABLE tool_calls ADD COLUMN prompt_trace   TEXT;
ALTER TABLE tool_calls ADD COLUMN remote         INTEGER DEFAULT 0;
```

- settings 键：`approval_mode`（`auto` 默认 | `ask`）。
- 明文纪律：headers/env 落库明文（本机 SQLite，对齐云端 key 现状）；**所有返回给前端的读取路径必须脱敏**（`sk-…`/`Bearer …` → `••••末4位`），grep 验收沿 spec-m2 惯例。

## 2. 命令层（mcp_admin.rs）

| 命令 | 行为 |
|---|---|
| `mcp_server_list` | 全量列表（headers/env 脱敏） |
| `mcp_server_save` | 新增/编辑；transport 校验（stdio⇒command 必填、http⇒url 必填且 https/http）；保存后 enabled 置 0（编辑不改 enabled） |
| `mcp_server_remove` | 删除 + 断开连接；若 tool_cache 引用仅存于此 server 则一并失效 |
| `mcp_server_toggle` | 启停；启用时立即连接 + list_tools 回填 tool_cache；失败 → Err(mcp_unreachable) 且 enabled 回 0 |
| `mcp_server_test` | 连接测试（不落 enabled），返回可达性 + 工具数 |
| `mcp_server_tools` | 读 tool_cache（带 TTL 5 分钟，沿 calendar_cache 惯例；过期由调用方触发刷新） |

- 运行时连接池：`AgentState { clients: Mutex<HashMap<server_id, McpClient>> }` managed state；toggle/save/remove 时同步增删；`RunEvent::Exit` 全量 shutdown。

## 3. 测试清单（CI，~8 测）

1. 迁移幂等：v7 库升 v8、重复 migrate 不炸、旧数据无损（2）→ `v7_database_upgrades_to_v8_idempotently`、`v1_database_migrates_losslessly`
2. save 校验：stdio 缺 command 拒、http 缺/坏 url 拒（2）→ `save_validation_rejects_bad_transport_fields`、`transport_from_row_validates_fields`
3. toggle：启用失败回 0（2）→ `toggle_flow_with_mock_client`；成功后 tool_cache 回填需真实 server，走 S7 真机
4. 脱敏：list 返回不含明文 token（1）→ `save_list_masks_secrets`
5. remove：断开 + 行消失（1）→ `remove_deletes_row_and_reports_missing`、`update_missing_row_errors_loudly`（行不存在须显式报错）

## 4. 验收清单

- [x] v7→v8 迁移幂等，cargo test 全绿
- [x] grep 确认：headers 明文只存在于写入路径，读取返回脱敏值
- [ ] 真机：设置页添加 stdio server → 启用 → 工具清单出现
- [ ] 真机：重启后 enabled/tool_cache 持久

## 5. 本机自动发现（mcp_discover.rs，2026-11-20）

MCP 协议没有本地服务发现标准：stdio 服务没有配置就无法发现（命令 + 魔法参数如
`prune mcp` 从二进制推断不出来），端口探测 / mDNS 不做。因此「自动发现」=
**扫描本机各客户端配置文件 + 通用 manifest sweep → 候选一键导入**。

- 配置源（硬编码，按优先级）：Cursor `~/.cursor/mcp.json`、Claude Desktop
  `~/Library/Application Support/Claude/claude_desktop_config.json`、Claude Code
  `~/.claude.json`（只取全局 `mcpServers`）、VS Code / Insiders `…/Code/User/mcp.json`
  （`mcpServers` 与 `servers` 两种键都认）、Windsurf `~/.codeium/windsurf/mcp_config.json`。
- 通用 sweep：`~/Library/Application Support/*/mcp*.json` 与 `~/.config/*/mcp*.json`
  一层深度、目录上限 200；known source 已覆盖的目录跳过。
- 解析：`command`+`args`+`env` → stdio，`url`+`headers` → http；支持
  `${env:NAME}` / `${NAME}` / `~/` 展开（缺变量保留字面量，不静默置空）。
  无法识别的条目跳过，单个坏文件不中断整个扫描。
- 去重签名：stdio = `stdio|{command}|{args.join(\u001f)}`，http = `http|{url}`，
  与 `mcp_admin::row_signature` 逐字节一致；同签名多来源保留首个（known source 标签更准）。
- 命令：`mcp_discover_scan`（候选 + imported 标记，env/headers 脱敏）、
  `mcp_discover_import(signatures)`（按签名**后端重扫落盘**导入，明文不经过前端
  IPC；导入行 `enabled=0`，RK-04 不破）。启用仍走手动 toggle + connect 验证。
- UI：工具页「扫描本机」→ 候选弹窗（已导入的禁选）→ 勾选导入。

验收：

- [x] cargo test 290 全绿（新增 6：cursor/http 解析、坏条目跳过、变量展开、
      签名与 row_signature 一致、known+manifest 扫描去重、导入停用与跳重）
- [x] vitest 75 全绿（新增 1：扫描弹窗渲染 + 按 signature 导入 + 重扫刷新）
- [ ] 真机：本机 Cursor 5 个 server 扫出 → 导入 → prune 启用成功

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

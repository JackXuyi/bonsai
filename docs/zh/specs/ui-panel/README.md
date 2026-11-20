# ui-panel specs — 索引

> 拆自 `docs/zh/plan-ui-panel.md`。每份 spec 独立可实施、独立可验收；按序执行，M0a → M0b → M1 → M2 →（M2.1 可选）→ M3 / M4 可并行 → M5 收尾。
> 公共约定见 §A；各 spec 不重复，只引用编号。

| Spec | 里程碑 | 依赖 | 状态 |
|---|---|---|---|
| [spec-m0a-nextjs-migration.md](spec-m0a-nextjs-migration.md) | M0a 前端迁移 Next.js | 无 | 已实施 |
| [spec-m0b-layout-shell.md](spec-m0b-layout-shell.md) | M0b 侧栏布局壳 | M0a | 已实施 |
| [spec-m1-local-model.md](spec-m1-local-model.md) | M1 本地模型配置总线 + 下载 | M0b | 已实施（待真机验收） |
| [spec-m2-cloud-model.md](spec-m2-cloud-model.md) | M2 云端配置清单 + ChatClient | M1 | 已实施（待真机验收） |
| [spec-m2-1-claude.md](spec-m2-1-claude.md) | M2.1 Claude 客户端（可选） | M2 | 已实施（待真机验收） |
| [spec-m3-memory-tab.md](spec-m3-memory-tab.md) | M3 记忆 tab | M0b（与 M1/M2 并行） | 已实施（待真机验收） |
| [spec-m4-persona-logs.md](spec-m4-persona-logs.md) | M4 人格卡片 + debug 日志 | M0b（与 M1/M2 并行） | 待实施 |
| [spec-m5-acceptance.md](spec-m5-acceptance.md) | M5 真机验收 + 文档 | 全部 | 待实施 |

## §A 公共约定

- **A1 语言与命名**：Rust 命令参数/返回一律 serde `#[serde(rename_all = "camelCase")]`，与前端 TS 类型对齐；TS 类型定义在 `frontend/src/lib/types.ts`，字段与 Rust struct 一一对应，禁止前端自造第二份含义。
- **A2 命令注册**：所有新命令加进 `lib.rs` 的 `invoke_handler`；命令函数体保持薄——纯逻辑下沉到可测函数，命令层只做取 State + 调用 + map_err。
- **A3 设置读写**：复用 `memory::db::{get_setting, set_setting, remove_setting}`（schema v7 settings 表，key/value/updated_at），**全程不升 schema**。新 key 一律 `llm_` 前缀。
- **A4 事件命名**：沿用现有点分风格（`llm:pull:progress`）；前端监听统一走 `lib/tauriListen.ts`，组件卸载必须 `safeUnlisten`。
- **A5 前端文件规范**（用户规则）：单文件 ≤500 行；复用 ≥2 次且 >10 行的逻辑提取 lib/hooks；公共组件必有 RTL/vitest 测试。
- **A6 禁改清单**：危机拦截 / 未成年门禁 / q4 抽取拦截的调用前位置；对话管线顺序（危机→工具→模型）；sidecar 生命周期（11435）；现有 162 个 Rust 单测不得退化。
- **A7 验收门槛**：每个 spec 完成需同时满足——cargo check 零警告 + cargo test 全绿 + `pnpm test` 全绿 + `pnpm app` 真机走通本 spec 的验收清单。

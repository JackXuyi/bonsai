# plan-db-split — 拆分 `memory/db.rs` 巨石

| 文档信息 | 内容 |
|---|---|
| 版本 | v0.1（已实施） |
| 日期 | 2026-11-24 |
| 状态 | **已实施**（T1–T3） |
| 上游输入 | 优化盘点 P2-11；`plan-dialogue-split`（只搬家范式）；`plan-memory` / `plan-store-lock` |
| 定位约束 | 行为零变化；`memory::db::*` 路径零变化；不夹带 SQL / schema 改动 |
| 一句话 | 按表域把 ~3900 行 `db.rs` 还原成 `db/` 目录——只搬家，不改脑子 |

---

## 0. 结论

`memory/db.rs` 堆了 schema 迁移、事件/事实写入、会话、设置、预测/成长/关系、日历与工具审计、任务卡、对话读出与 turn_trace、检索。导航与冲突成本已高于功能本身。

对齐 `plan-dialogue-split`：**目录化 + `pub use` 保对外路径**；测试整块留在 `db/tests.rs`。

### 取舍

| # | 裁决 | 理由 |
|---|---|---|
| D1 | `db/` 目录，不 `db_*.rs` 平铺 | 与 `dialogue/` / `llm/` 一致 |
| D2 | 绝对不改 SQL / 迁移语义 | 拆分与修 bug 缠在一起无法回滚 |
| D3 | 对外仍 `crate::memory::db::foo` | 全仓 ~80 处调用面零改 |
| D4 | 测试不按域再拆（V1） | 生产代码已降冲突；测试二次拆可另案 |

---

## 1. 布局

| 文件 | 域 |
|---|---|
| `schema.rs` | `SCHEMA` / `migrate` / `open` / `open_reader` / `PROACTIVE_SESSION_ID` |
| `events.rs` | events/facts/boundaries/preferences 写、forget、consolidate 读、窗口统计、search |
| `sessions.rs` | 会话索引 + digest |
| `settings.rs` | settings KV + onboarding 答卷 |
| `progress.rs` | predict / growth / relationship |
| `audit.rs` | calendar 缓存、tool_calls、tasks / plan_json |
| `chat.rs` | dialogue 历史读出、turn_trace |
| `tests.rs` | 原 `mod tests`（35） |
| `mod.rs` | `pub use` 聚合 |

---

## 2. 里程碑

| # | 内容 | 验收 |
|---|---|---|
| T1 | 按域切文件 + 修跨模块路径 | 可编译 |
| T2 | `memory::db` 单测全绿；调用面零改 | 35 passed |
| T3 | 本文回填 | 文首已实施 |

---

## 3. 变更记录

| 版本 | 日期 | 说明 |
|---|---|---|
| v0.1 | 2026-11-24 | 初稿并落地 T1–T3 |

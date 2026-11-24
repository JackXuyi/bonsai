# 方案：P3-5 常驻任务框架（统一 jobs）

| 文档信息 | 内容 |
|---|---|
| 版本 | v1 |
| 日期 | 2026-11-24 |
| 状态 | **已实施**（与 Phase 3 A5 同表；本版完成提醒 / 日报迁入） |
| 上游 | [`roadmap-agent-capabilities.md`](../../roadmap-agent-capabilities.md) P3-5；[`plan-autonomy-phase3/v1`](../../plan-autonomy-phase3/v1/index.md) §8 |
| 一句话 | **一张 jobs 表 + 一个 scheduler tick；执行体仍在各权威模块，不内嵌脚本** |

---

## 0. 范围

| Job `name` | schedule | 执行体 | 说明 |
|---|---|---|---|
| `consolidate` | `daily@03:00` + 36h catchup | `consolidate::` | 既有夜间巩固；失败 ≤3 + JobGuard 退避 |
| `calendar_remind` | `interval:60` | `calendar::tick` | 从独立 remind 线程迁入；原 `calendar` loop 只保活心跳（齐套 8 行） |
| `daily_digest` | `daily@21:00` | `report::build` | 每日一次生成日报落 settings + emit；无活动可 skipped |

非目标：完整 cron 解析库、用户自定义脚本引擎、把 predict/growth/relationship 强行并进（另议）。

---

## 1. 不变量

1. **调度一份权威**：到期判定只读 `jobs`；禁止 remind / consolidate / digest 再各开互不知晓的 fire 路径。
2. **执行一份权威**：consolidate / calendar / report 逻辑不复制。
3. **失败有界**：`consecutive_fails < 3` 才跑；达上限保持 enabled 但 tick skip + 日志。
4. **不阻塞 UI**：长任务零锁跑 LLM（沿用既有 narrowing）。
5. **loop 齐套**：`calendar` 名仍在 `LOOP_NAMES`；提醒副作用只在 scheduler。

---

## 2. 验收

| ID | Given / When / Then |
|---|---|
| J1 | 新增/种子 job 后，到达 schedule → 执行且 `last_status`/`last_run_at` 更新 |
| J2 | 连续失败 3 次 → 本 job 本进程内不再跑，直到人工或 `record_ok` |
| J3 | `calendar_remind` 由 scheduler 触发；`calendar::spawn` 不再 `emit calendar:remind` |
| J4 | `daily_digest` 同一本地日不跑第二次 |
| J5 | `job_list` / `job_set_enabled` 可查可关 |

---

## 3. 变更记录

| 版本 | 日期 | 说明 |
|---|---|---|
| v1 | 2026-11-24 | 初稿并实施：三内置 job + 统一 tick + 命令 |

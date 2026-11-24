# 方案：P4-4 能力线接入任务卡

| 文档信息 | 内容 |
|---|---|
| 版本 | v1 |
| 日期 | 2026-11-24 |
| 状态 | **已实施** |
| 上游 | [`roadmap-agent-capabilities.md`](../../roadmap-agent-capabilities.md) P4-4；[`plan-planning.md`](../../plan-planning.md) §5.7；调研 §6.2/6.3 |
| 定位约束 | 靠「办成事」的理解/执行质量推进，不靠签到；分数永不外露；与形态/关系线节奏错开 |
| 一句话 | **任务卡全部校验通过 → 当日能力证据 +1；失败 −1；取消不扣；阶段双条件（天数+累计）** |

---

## 0. 范围

| 做 | 不做 |
|---|---|
| `ability` / `ability_progress` / `ability_log`（schema v21） | 经验条 / 成就徽章 UI |
| 从 `tasks` **重算**日证据（含 `finished_at`） | 在 finish 路径累加计数器 |
| 「全部校验通过」= `done` 且（无计划 或 全部 step=`done`） | 把预测 hit 混进能力线（仍归关系线） |
| `ability_status` + 设置/工具卡文案 | 第 9 条后台 loop（挂在 relationship tick 后） |
| 兑现 plan-planning §5.7 证据出口 | 改任务卡执行引擎语义 |

---

## 1. 阶段（双条件）

| 阶 | 名 | min_day（自 born_at） | min_cum（不含今日） |
|---|---|---|---|
| 0 | 陪聊 | 0 | 0 |
| 1 | 能办事 | 31 | 15 |
| 2 | 真帮手 | 60 | 40 |

---

## 2. 日证据（内部，不外露）

| 信号 | 判定 | 分 |
|---|---|---|
| present | 当日有已结算任务（`finished_at` 落在本地日） | +1 |
| validated | ≥1 张全部校验通过 | +1 |
| failed | ≥1 张 `failed` | −1 |
| canceled | ≥1 张取消 | 0（不扣，对齐 shut_up 哲学） |

`score ∈ [-1, 2]`。重算 UPSERT，可回放。

---

## 3. 验收

| ID | Given / When / Then |
|---|---|
| A1 | 计划任务全部 step=done 且 status=done → 当日 validated≥1 |
| A2 | status=failed → misses，当日 score 可 −1 |
| A3 | 同日源数据不变 → progress UPSERT 幂等 |
| A4 | day&cum 双门槛未齐 → 不升阶 |

---

## 4. 变更记录

| 版本 | 日期 | 说明 |
|---|---|---|
| v1 | 2026-11-24 | 初稿并实施：ability 模块 + 任务结算证据 + UI 卡 |

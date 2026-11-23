# 对话 IPC 超时修复（P0/P1/P2）

> 2026-11-23。起因：真机一条消息报 `（出错了：Error: [IPC] dialogue_chat_stream did not respond within 15000ms）`。
> 本文记录根因、三层修复与新的流式协议契约；后续改动 `useChat` / `dialogue_chat_stream` 前先读第 4 节。

## 1. 现象

- 用户发一条「黄山未来一周天气」类多步问题，约 15 秒后气泡被撤、出现 IPC 超时错误，但**后端仍在继续跑**并最终把回复落进 `events`。
- 工具页计划进度还在推进，对话区已经显示失败——前后端各说一套。
- 超时后 `chatting` 复位，用户可立刻再发一条，与仍在后台跑的那轮并发。

## 2. 根因

| 环节 | 事实 | 位置 |
|---|---|---|
| 前端兜底 | 所有 `invoke` 默认 **15s** 固定超时（`Promise.race`），不区分命令语义 | `frontend/src/lib/tauriInvoke.ts:3` |
| 命令语义 | 旧 `dialogue_chat_stream` 是「整轮跑完才 resolve」 | `dialogue.rs`（本次改为 fire-and-forget） |
| 单轮真实耗时 | 计划轮 1 次 HTTP + 每步工具 2 次 + 单次 HTTP 上限 **120s** + 有界重试 | `llm/mod.rs:40`、`dialogue.rs:361/363`、`RETRY_BACKOFF` |
| 实测 | 12:49:38 发起 → 12:50:06 已 28s 且未完；单次云端 HTTP 就有 10s；两次 `web_search` 各 9s/7s（DDG IPv6 死等 + 同 query 重试） | `~/Library/Logs/app.bonsai.desktop/bonsai.log` |

结论：**15s 的固定上限与「分钟级」命令语义不匹配**。T2（计划轮）、T4（重试）把单轮从「一句话回复」变成「分钟级任务」后必然撞线。

## 3. 三层修复

| 层 | 改动 | 结果 |
|---|---|---|
| **P0** 止血 | ① 对话命令单独给 10 分钟 IPC 兜底（只防真挂死）；② 超时不再撤已流出的气泡，追加「可能还在后台跑」；③ 新增 `dialogueCancel(taskId)` 封装 + 输入区「停」按钮 | 误判消失；用户可主动叫停后端 |
| **P1** 根治 | `dialogue_chat_stream` 改 **fire-and-forget**：命令立即返回，正文与终态全走 Channel（`turn_done` / `turn_error`）；前端改为事件驱动，兜底换成「事件静默 150s」看门狗 | 命令时长与业务时长解耦，跑 10 分钟也不会被 IPC 判死 |
| **P2** 放大器 | ① 搜索用独立 agent：connect 4s / total 10s（抓取仍 10/20）；② `web_search`/`web_fetch` **不参与 read 重试**；③ 回合进行中再发被忽略（单测锁定） | 单轮少 7-9 秒无效等待，不并发跑两轮 |

## 4. 流式协议契约（改动前后必读）

`dialogue_chat_stream(userText, sessionId, onDelta)`：

- **返回值**：`void`——只表示「已受理」，不再是整轮结果。
- **Channel 事件**（同一 `onDelta`）：
  | 事件 | 载荷 | 语义 |
  |---|---|---|
  | `delta` | `string` | 正文增量 |
  | `task_step` | `{event,id,seq?,tool?,status?}` | 任务/计划进度（只转 `planStore`，不进消息流） |
  | `turn_done` | 完整 `DialogueOutcome` | **整轮唯一权威终态**（含 `reply` / `turn_id` / `filter`） |
  | `turn_error` | `string` | 管线错误（模型不可达等） |

- 前端不变量：`chatting` 从发送起保持到 `turn_done`/`turn_error`/静默超时；`task_*` 永不写入 `chatStore.messages`（A15）。
- 新增命令/事件时同步改 `frontend/src/hooks/useChat.ts` 的 `StreamMsg` 与 `asEvent` 风格的容错解析，不要在组件里各自猜形状。

## 5. 验收

- 门禁：`cargo test --lib` **470 passed**（新增「外部网络类不重试」）；`tsc --noEmit` 0 错；`vitest run` **170 passed**（新增：超时不撤气泡 / turn_error / 并发忽略 / 单步任务不进面板）。
- 真机建议复跑同一句「黄山未来一周天气」：预期气泡一直留着、工具页进度推进、最终拿到回复；中途点「停」→ 本轮 `canceled` 且剩余步骤 `skipped`。

## 6. 遗留

- 窗口重载/关闭会丢 Channel，后台那轮的结果只落库不回显（下次回放可见）——可接受，未做补偿。
- 后端对同一 session 的并发回合**未显式串行化**；当前靠前端「一轮未结束不允许再发」保证。若将来做多端/多窗口，需要后端加 session 级队列。
- `web_search` 仍只有 DuckDuckGo 一个来源（plan-web.md 的 v1 决策）；P2 只是收敛超时，未新增 provider。

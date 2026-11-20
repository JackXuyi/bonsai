# spec-s7 — 真机验收 + 文档收尾

> 依赖：S6。目标：全链路真机回归（三级管线成本 / 2B 误触发率 / 人格漂移 / 审计可查）+ 文档同步。

## 1. 三级管线与成本

- [x] 无候选路径仅 1 轮模型调用（CI：`pipeline_degrades_silently_on_garbage` 断言零额外轮次、零审计行）
- [ ] 真机：无候选路径对话延迟与 v7 时代无可感差异
- [x] 全链路 3 轮：候选 → 详情 → 结果作答（CI：`pipeline_three_rounds_full_chain_is_audited`）
- [ ] 真机：全链路本地 2B 延迟可接受（≤15s 主观阈值）
- [ ] 真机：2B 误触发率——30 样本评测集（10 应触发 / 20 不应触发），不应触发误选率 ≤20%（RK-01 对齐）；超标 → 触发 Plan B 评审（手动触发词）
- [x] 「今天有什么会」仍走日历规则路由，管线不抢既有意图（CI：`kit_never_preempts_crisis_or_calendar_routing`）

## 2. MCP 双传输

- [ ] 真机：stdio server（filesystem 或同类）连接 / 工具清单 / 调用 / 退出无孤儿进程
- [ ] 真机：http server 连接 / 调用 / 断网显式报错 mcp_unreachable（非空结果非挂死）
- [x] 添加 http server 时隐私提示条出现且不可跳过（CI：`form shows the privacy hint for http transport`；提示为常驻文案，无关闭入口）

## 3. Skill

- [ ] 真机：示例包导入 → 启用 → 对话风格/知识生效；停用即失效
- [x] 超限/注入样本包拒载并标红（CI：`oversized_pack_is_rejected`、`injection_blacklist_rejects_all_samples`、`invalid skill shows the rejection reason`）
- [x] 人格回归：启用 skill 后危机样本仍拦截（CI：`kit_never_preempts_crisis_or_calendar_routing`）
- [ ] 真机：关系线语气档在启用 skill 后不变（需真机对话观察）

## 4. 审批与审计

- [x] approval_mode=auto：调用直接执行，审计行 result_status=auto_allowed（CI：`execute_audited_records_ok_and_failure_status`）
- [x] approval_mode=ask：无 UI 应答时拒绝，审计行 denied_by_gate（CI：`ask_mode_gate_denies_when_no_ui_is_attached`、`approval_broker_handshake`）
- [ ] 真机：approval_mode=ask 时审批卡弹出 → 允许 → 工具执行成功；拒绝 / 30s 超时均拒绝
- [ ] 真机：调用记录卡与 tool_calls 实况一致；prompt_trace 可定位到 bonsai.log 对应片段

## 5. 文档收尾

- [x] `plan-agent-mcp-skill.md` 状态行更新为已实施 + 偏差记录（§8，5 条偏差 + 3 条接手约束）
- [x] `README.md` / `README.zh-CN.md` 功能清单补：MCP 工具、Skill 包、调用审计（并把过期的 Vite / 三页描述改成 Next.js / 五页）
- [x] 工作区记忆当日记录补实施摘要
- [x] roadmap G-07/G-11/G-12 状态更新（P3-4 提前落地部分；P3-5 的 job 表顺延到 schema v9）

## 6. 退出标准

全部勾选 + cargo test / pnpm test 全绿 → plan-agent-mcp-skill 关闭；roadmap P3 剩余项（邮件只读 P3-1、文件 P3-2、Web 检索 P3-3、job 表 P3-5）照 roadmap 推进，P3-2/P3-3 直接实现 ToolProvider 挂进本管线。

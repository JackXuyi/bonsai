# spec-s6 — UI 交互

> 依赖：S3–S5。目标：设置页两张新卡 + 工具页调用记录卡 + 对话内工具标注 + 审批确认卡组件（预留）。沿 settings 页现有卡片惯例（ModelCard / CloudSection 样式 token）。

## 1. 设置页：MCP 卡片（McpSection.tsx）

- 列表：每行 server 卡（名称 + transport 徽标 `stdio`/`http` + enabled 开关 + 工具数 + 最近 tool_cache 时间）。
- 添加/编辑弹窗（CloudFormModal 同款）：
  - transport 单选：stdio（command / args）/ http（url / headers 键值对）
  - 传输校验错误就地显示（对齐 humanizeLlmError 风格，新错误码进 humanizeAgentError）
  - **http 必显隐私提示条**：「该工具由外部服务处理，对话参数会发往 <url>」（v0.4 D12）
- 动作：测试连接（mcp_server_test）→ 成功显示工具数；删除（确认弹窗）。
- 展开行：工具清单（name + summary，来自 tool_cache，TTL 内直接读）。

## 2. 设置页：Skill 卡片（SkillSection.tsx）

- 列表：skill 名 + description 一行 + enabled 开关；invalid 包标红 + 错误码文案。
- 导入：MVP 不做目录/zip 选择器——把包放进 skills 目录后刷新列表即生效（`skill_list` 每次实时扫描）。
  **偏差**：原计划的「重新扫描」按钮（`skill_reload`）取消，理由见 spec-s5 §4：无缓存可失效，按钮只会是 `skill_list` 的同义操作。
- **偏差**：原计划「全文 >15KB 黄条警告」取消，改为管线硬约束（同时注入 ≤2 个 skill 全文，单包 16KB + 5×4KB，见 spec-s5 §3 及其 2026-11-23 偏差注记）。

## 3. 工具页：调用记录卡（ToolCallRecords.tsx）

- 最近 20 条倒序：时间 / kind 徽标（mcp/skill）/ action 名 / outcome 状态点（ok 绿 / denied 橙 / error 红） / 耗时 / remote 标。
- 点开行：params_summary + result_status 全文（prompt_trace 引用一并列出）。
- 只读；数据源 `tool_call_records` 命令，进页拉取 + 手动刷新按钮。

## 4. 对话内标注

- `DialogueOutcome.tool` 前缀 `mcp:` / `skill:` → 气泡 meta 行显示「🔧 经 <name>」（日历 tool 标注同款样式，不区分本地/远程颜色，远程在记录卡看）。

## 5. 审批确认卡（ApprovalCard.tsx）与模式开关

- 监听 `tool:approval_required` 事件：卡片显示 tool / 来源 / remote 提示 / 参数摘要 + 「允许本次 / 拒绝」。
- 回执走 `mcp_approval_reply(id, allow)`；30s 无操作为后端超时拒绝。
- 回执返回 `false`（已超时或已答复）→ 卡片留在「已过期」态，只留「知道了」，不再重复回执。
- 模式开关在 **设置页 MCP 卡片顶部**（`agent_approval_get` / `agent_approval_set`，取值仅 `auto|ask`）；默认 `auto`，卡片不弹。
  **偏差**：原设计的第三按钮「本会话不再询问」未实现——它等价于把 `ask` 悄悄改回 `auto`，会造成「我明明选了每次询问」的落差；当前只保留单次允许/拒绝，会话级免打扰等 P4 权限分级时一起做。

## 6. 测试清单（vitest + RTL，~6 测）

1. McpSection：列表渲染 + enabled 开关触发命令（mock invoke）（1）→ `renders rows with transport badge and actions`、`toggling enable calls the command`
2. transport 切换：stdio/http 表单字段联动 + 校验错误显示（1）→ `form switches fields between stdio and http`
3. 隐私提示条：transport=http 且填 url 后出现（1）→ `form shows the privacy hint for http transport`
4. SkillSection：invalid 包标红 + 错误码文案（1）→ `invalid skill shows the rejection reason`
5. ToolCallRecords：mock 数据渲染 + 状态点颜色（1）→ `records card renders outcome dots and details`
6. ApprovalCard：事件 → 卡片出现 → 回执命令参数正确（1）→ `tests/ApprovalCard.test.tsx`（5 测：不弹/弹卡/允许/拒绝/过期）
7. 审批模式开关：读取当前值 + 切换落库 + 文案随之变化（1）→ `approval mode switch persists the choice`

## 7. 验收清单

- [x] 上述 vitest 全绿；tsc --noEmit 干净；next build 过
- [ ] 真机：MCP 添加→测试→启用→工具清单展开 全流程
- [ ] 真机：skill 导入→启停→标红拒载显示
- [ ] 真机：调用记录卡与后端实况一致；对话 🔧 标注正确
- [ ] 真机：approval_mode 切 ask 后审批卡弹出→允许→执行成功

# spec-m2.1 — Claude 客户端（可选，挂起）

> 依赖：M2。触发条件：用户明确要 Claude 官方 API。不做不扣分。

## 1. 范围

- `CloudClient` 之外新增 `AnthropicClient`：`POST {base}/v1/messages`，header `x-api-key` + `anthropic-version: 2023-06-01`；body `{ model, max_tokens, system, messages }`（system 独立字段，从 messages 里拆出 role=system 的首条——bonsai 的 persona system prompt 恰好走这里）。
- 流式：SSE `content_block_delta` → `delta.text`。
- `CloudEntry.kind` 增加 `Claude`，base_url 默认 `https://api.anthropic.com`。
- 前端：CloudFormModal 的 Claude 选项解禁，端点字段默认值同上。
- `current_client()` 工厂按 kind 分支。

## 2. 测试

- `parse_anthropic_sse_line`：delta text / message_stop / ping / error（4）
- 请求体构造：system 拆分正确（system prompt 单独字段、messages 无 system 残留）（2）
- 工厂 claude 分支（1）

## 3. 验收

- [ ] 添加 Claude 条目 → 测试 → 切换 → 流式对话正常 → 危机样本照常拦截

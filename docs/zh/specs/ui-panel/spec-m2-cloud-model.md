# spec-m2 — 云端模型：配置清单 + ChatClient 抽象

> 依赖：M1。目标：云端配置清单（多实例）+ 添加/编辑弹窗 + opt-in 警示 + 传输层抽象。**危机拦截/未成年门禁/q4 拦截在模型调用前，本 spec 不触碰它们的代码路径（§A6）。**

## 1. Rust：传输层抽象（`src-tauri/src/llm.rs` 改造）

### 1.1 ChatClient trait

```rust
pub trait ChatClient: Send + Sync {
    fn chat(&self, msgs: &[Msg], temperature: f64, timeout: u64) -> Result<String, LlmError>;
    fn chat_stream(&self, msgs: &[Msg], temperature: f64, timeout: u64,
                   on_delta: &mut dyn FnMut(&str)) -> Result<String, LlmError>;
    fn is_up(&self) -> bool;
}
```

- `OllamaClient` 平移实现（现有方法签名本就一致，impl 挂 trait 即可，`from_env`/`new` 保留）。
- 新 `CloudClient { base_url, model, api_key }`：
  - `POST {base_url}/chat/completions`，header `Authorization: Bearer <key>`；body：`{ model, messages, stream, temperature, max_tokens: 4096 }`（OpenAI 兼容统一覆盖 DeepSeek/Moonshot/GLM/Qwen 兼容模式/OpenRouter/OpenAI 官方）。
  - 流式：SSE——纯函数 `parse_sse_line(line) → Option<String>`（`data: [DONE]` → None、`data: {...}` 取 `choices[0].delta.content`、空行/注释行 → None），与 `parse_stream_line` 同风格带测。
  - 错误映射：非 2xx → `LlmError::Status(code)`，body 里 JSON error.message 拼进日志。
- **工厂**：`llm::current_client() -> Arc<dyn ChatClient>`——读覆盖层：provider=ollama → OllamaClient；provider=cloud → 按 active_id 从 `llm_cloud_entries` 找条目构造 CloudClient（条目缺失 → 回落 OllamaClient 并 log::warn）。
- **4 个调用点改造**（唯一本 spec 触碰的旧代码）：`persona/eval.rs` LocalBackend、`memory/extract.rs` LocalExtractor、`report.rs`、`dialogue.rs` 的构造——`OllamaClient::from_env()` → `llm::current_client()`。**DialogueBackend trait（persona/eval.rs）不动**——ChatClient 在它下一层。
- 抽取腿兼容：云端模型对结构化 JSON 指令同样生效；解析失败回落 RuleExtractor 的既有链路原样工作（不新增代码，回归验证）。

## 2. Rust：云端配置清单 CRUD（并入 `llm_admin.rs`，注意 500 行上限——若超，拆 `llm_admin.rs` + `llm_admin_cloud.rs`）

### 2.1 数据结构

```rust
#[derive(Serialize, Deserialize)]
pub struct CloudEntry { pub id: String, pub name: String,
    pub kind: EntryKind,          // CustomOpenAi | OpenAi
    pub base_url: String, pub model: String, pub key: String }
```

- 存储：settings 表单 key `llm_cloud_entries` = JSON 数组（§A3；不建新表，条目量个位数）。
- **脱敏铁律**：所有对外返回（get/list/entries_get）key 只回 `mask_key()`（`••••`+末4位，≤4 位全遮）；前端编辑回填脱敏值，用户重输即覆盖（空值 = 保持原 key）。

### 2.2 命令

| 命令 | 签名 | 校验 |
|---|---|---|
| `llm_cloud_entries_get` | `→ Vec<CloudEntryView>`（key 脱敏） | — |
| `llm_cloud_entry_save` | `entry: CloudEntryInput → CloudEntryView` | kind=OpenAi → base_url 强制 `https://api.openai.com/v1`；URL 必须合法 http(s)、key/model/name 非空；带 id = 编辑（id 不存在 → Err）；**不做连接测试**（显式测试按钮触发 `llm_cloud_test`） |
| `llm_cloud_entry_remove` | `id → Result` | id 为 active_id → 联动回落：`llm_active_id` 改 `ollama:<默认模型>`、`llm_provider` 改 ollama（防锁死） |
| `llm_cloud_test` | `entry: CloudEntryInput → Result<TestReport { latency_ms }, String>` | 1-token chat 请求；不落库 |
| `llm_config_set_active` | M1 已有，扩 id 语义：`cloud:<entry_id>` → 条目存在 → 读条目构造 CloudClient 测试 → 写 `llm_provider=cloud` + `llm_active_id`；失败拒绝切换 | — |

## 3. 前端

```
components/settings/
├── CloudSection.tsx        # 清单 + 添加按钮
├── CloudEntryCard.tsx      # 名称/类型 tag/模型/端点/脱敏 key/测试✓/× 删除
├── CloudFormModal.tsx      # 添加/编辑弹窗（shadcn Dialog）
└── CloudOptinModal.tsx     # 警示模态（每会话一次）
```

- **顶部选择器（M1 已有本地区，本 spec 加双 tab）**：`本地 | 云端` 段选（`uiStore` 扩展 `modelTab`）。云端 pane = 清单列表。
- **点卡片即选中**：`llm_config_set_active("cloud:"+id)`；首次选中任一云端条目前弹 `CloudOptinModal`（每会话记住一次，`uiStore.cloudOptinAcked`）；「还是本地」→ 不切；确认 → 继续切换流程。
- **添加弹窗字段**：名称 / 类型 select（自定义 OpenAI 兼容｜OpenAI 官方｜Claude（即将支持，disabled））/ 接口地址（官方时锁定隐藏）/ API Key（明文输入）/ 模型名；「测试连接」内联结果（✓ 测试通过 · 1 token · 0.4s）；「保存」→ `entry_save` → 回清单。
- 编辑复用弹窗：预填脱敏 key，placeholder 提示「留空保持原 key」。
- 删除激活条目：确认文案提示「将回落到本地默认模型」。
- 页头状态：云端激活时显示「云端 · <条目名>」+ 灰点改黄点（区分本机绿点）。

## 4. 边界与安全（复查项，写进 PR 描述）

1. 云端 opt-in 默认关闭；警示文案三条（文本上传 / 人格记忆不出机器 / key 明文）。
2. 危机词拦截、未成年门禁、q4 拦截均在 `chat_turn` 模型调用**之前**，与 provider 无关——不改，回归验证 crisis 样本在云端模式下仍出资源卡。
3. 反谄媚 filter（generation.rs 本地规则）照过云端回复——回归 `EXPLICIT_DISAGREEMENTS` 样本。
4. key 明文存储的 UI 提示已在 opt-in 文案中；`llm_cloud_entries_get` 任何路径不得返回明文（测试锁定）。

## 5. 测试清单

Rust（cargo test，~19）：
1. `parse_sse_line`：content delta / [DONE] / 空行 / 注释行 / 无 choices / error JSON（6）
2. `CloudClient` 请求体构造（model/messages/stream/temperature/auth header）（2，mock server 或纯构造断言）
3. `mask_key`：长 key 留末 4 / 短 key 全遮 / 空（3）
4. entries CRUD：save 生成 id / 编辑保 id / OpenAi 强制 base_url / 非法 URL 拒绝 / remove active 回落 ollama（5）
5. `current_client` 工厂：ollama 分支 / cloud 分支 / 条目缺失回落 + warn（3）

前端（vitest + RTL，~9）：
1. `CloudEntryCard`：脱敏显示 / active 态 / 删除按钮（2）
2. `CloudFormModal`：官方类型锁定端点 / 校验空值 / 测试结果渲染（3）
3. opt-in：首次弹 / acked 后不再弹 / 取消不切（3）
4. `llmStore` modelTab 切换（1）

## 6. 验收清单

- [ ] 添加 DeepSeek 条目：测试 ✓ → 保存 → 点卡片 → opt-in（首次）→ 生效；发消息验证流式（SSE）与 done 元数据 provider 标注
- [ ] 抽取链路：夜间 consolidate 用云端模型成功产事件；故意填错模型名 → RuleExtractor 回落、事件不丢
- [ ] 删除激活条目 → 自动回落本地默认模型，页头状态正确
- [ ] 危机样本在云端模式下仍出资源卡；反谄媚样本仍被标注
- [ ] 重启持久：provider=cloud + active 条目恢复
- [ ] grep 确认：明文 key 只存在于写入路径，所有读取路径返回脱敏值

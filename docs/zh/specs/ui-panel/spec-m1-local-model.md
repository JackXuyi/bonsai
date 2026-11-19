# spec-m1 — 本地模型配置总线 + 模型下载

> 依赖：M0b。目标：模型选择进 UI（卡片点击即切换）、推荐目录下载、进程级热生效。Rust 新增 `llm_admin.rs`；前端填充 `/settings` 模型卡。

## 1. Rust：配置总线（`src-tauri/src/llm_admin.rs` 新模块，≤500 行）

### 1.1 覆盖层（进程级，热生效）

```rust
pub struct LlmConfig { provider: Provider, local_model: String, active_id: String }
pub enum Provider { Ollama, Cloud }   // Cloud 在 M2 才有真实分支，枚举先占位

static LLM_OVERRIDE: OnceLock<RwLock<Option<LlmConfig>>> = OnceLock::new();

// llm.rs 修改 from_env()（名字不改，注释声明优先级）：
//   1. LLM_OVERRIDE 有值 → 用之
//   2. env OLLAMA_HOST / BONSAI_LLM_MODEL（开发期习惯）
//   3. DEFAULT_BASE_URL / DEFAULT_MODEL
```

- 4 个调用点（`persona/eval.rs` LocalBackend、`memory/extract.rs` LocalExtractor、`report.rs`、`dialogue.rs` 内构造处）**签名不动**——`from_env()` 每次调用读覆盖层即热生效，无需重启 sidecar。
- 设置 key（§A3）：`llm_provider`（"ollama"|"cloud"）、`llm_local_model`、`llm_active_id`。
- 启动回填：`run()` setup 中读 settings，有值则写入覆盖层（UI 设置优先于 env，跨重启持久）。

### 1.2 命令（全部 camelCase 序列化，§A1）

| 命令 | 签名 | 行为 |
|---|---|---|
| `llm_config_get` | `→ LlmConfigView { provider, local_model, active_id, sidecar_up, masked_key? }` | 读覆盖层 + 探测 sidecar |
| `llm_config_set_active` | `id: String → Result<(), String>` | 本地 id 形如 `ollama:<tag>`：校验 tag 在 `/api/tags` 列表 → 测试（1-token chat）→ 写 settings 三键 → 更新覆盖层；失败 Err 不落库 |
| `llm_model_list_installed` | `→ Vec<OllamaModelRow { name, size_bytes, modified }> ` | 代理 sidecar `GET /api/tags`；sidecar 不可达 → Err("sidecar_down")（前端回落静态清单并显示灰点） |
| `llm_catalog` | `→ Vec<CatalogRow>` | 返回编译内置推荐目录（见 §3），纯函数可测 |
| `llm_model_download` | `tag: String → Result<(), String>` | 并发上限 1（见 §2）；spawn `ollama pull <tag>` 子进程，stderr 丢弃、stdout NDJSON 逐行解析 |
| `llm_model_remove` | `tag: String → Result<(), String>` | `ollama rm`；tag == 当前 active → Err("model_in_use")；无确认（前端做） |

- **下载子进程管理**：`PullState { child: Mutex<Option<Child>>, tag: Mutex<Option<String>> }` managed state；`RunEvent::Exit` 时随 ServerHandle 一并 kill（补进现有 exit 分支）。
- 进度事件：`llm:pull:progress` → `{ tag, status: "pulling"|"done"|"error", percent, completed, total, error? }`。percent 计算：`completed/total`（Ollama NDJSON 的 status/summary 字段），解析为纯函数 `parse_pull_line(line) → Option<PullProgress>` 带测。
- 并发语义：`PullState.child` 已有 Some → `Err("download_busy")`；不做队列（前端按钮转灰即可）。
- 磁盘预警：下载前查目标目录所在卷剩余空间 < 模型大小 ×1.2 → 事件 `llm:pull:progress` 先发 `status:"warn_disk"`，**继续下载不拦截**（裁决见 plan §5.6）。

## 2. 推荐目录（编译内置，纯数据）

```rust
pub struct CatalogRow { tag: &'static str, size_label: &'static str, size_bytes: u64,
    note: &'static str, think_ok: bool, ram_hint: Option<&'static str>, recommended: bool }
```

| tag | 大小 | think_ok | 备注 |
|---|---|---|---|
| qwen3.5:2b | ~0.8GB | ✅ | 当前默认 |
| qwen3.5:4b | ~2.4GB | ✅ | 16GB+ |
| gemma3:4b | ~3.3GB | ✅ | 思维链无 |
| llama3.2:3b | ~2.0GB | ✅ | 中英混合稍弱 |
| deepseek-r1:1.5b | ~1.1GB | ❌ | ⚠ 思维链不可关——**默认折叠进「更多」**（qwen3:4b 教训：污染正文） |

- 目录过滤：已装 tag 从「可下载」区剔除（前端比对）。
- ModelScope 下载源：**不做**（后续增强），目录 note 不提及镜像，避免空 promise。

## 3. 前端（`/settings` 模型卡，M0b 占位替换）

```
components/settings/
├── ModelSection.tsx        # 组装：状态行 + 两区
├── InstalledModelGrid.tsx  # 已装卡片 grid（点卡片即切换）
├── ModelCard.tsx           # 卡片：name/size/modified/tags/check；testing 态 spinner
├── DownloadList.tsx        # 可下载区：进度条 / 取消 / 完成后移入已装
└── hooks/useModels.ts      # 数据编排：list_installed + catalog + pull 事件订阅 → llmStore
lib/stores/llmStore.ts      # { provider, activeId, installed[], catalog[], pulls: Record<tag, PullProgress> }
```

- **点卡片即切换**（无保存按钮）：`llm_config_set_active("ollama:"+tag)` → 乐观置 testing 态 → 成功打 ✓ + 页头状态更新 + toast；失败还原 + 内联红字。
- 下载进度：订阅 `llm:pull:progress`（useTauriEvent）更新 `llmStore.pulls[tag]`；完成后调用 `list_installed` 刷新。
- 侧车不可达（`sidecar_down`）：已装区显示 Err 提示 + 静态 fallback 清单（目录中 recommended 项，只读不可选）+ 页头灰点。
- 删除：卡片右键（或长按）→ 确认 → `llm_model_remove`；`model_in_use` 错误 toast「先切换到其他模型」。

## 4. 测试清单

Rust（cargo test，~14）：
1. `parse_pull_line`：progress 行 / status-only 行 / 完成 / 错误 / 垃圾行（5）
2. 覆盖层：override 压过 env / 清空回落 / settings 回填启动（3）
3. `llm_config_set_active`：不存在 tag 拒绝 / 成功写三键 / 失败不落库（3）
4. `llm_catalog`：字段完整 + think_ok=false 的 recommended 必为 false（2）
5. `llm_model_remove`：active 拒绝（1）

前端（vitest + RTL，~7）：
1. `ModelCard`：on/testing/check 三态渲染（2）
2. `useModels`：set_active 失败还原 testing 态（1）
3. `DownloadList`：进度事件驱动 progress / done 后刷新已装（2）
4. `llmStore`：pull 快照覆盖 / 清除（2）

## 5. 验收清单

- [ ] 设置页下拉变卡片：点 qwen3:4b → 测试 → ✓ → 立即发消息验证走新模型（done 事件 model_used 字段）
- [ ] 下载 gemma3:4b：进度条推进 → 完成后出现在已装 → 点选生效；中途取消正常
- [ ] 并发保护：第二个下载点击提示 busy
- [ ] 重启应用：选择持久（settings），页头状态正确
- [ ] sidecar 关掉：设置页灰点 + fallback 清单，对话页显示「本地模型未就绪」
- [ ] `llm:pull:progress` 事件在 exit 时无泄漏（kill 后无孤儿 ollama pull 进程：`pgrep -f "ollama pull"`）

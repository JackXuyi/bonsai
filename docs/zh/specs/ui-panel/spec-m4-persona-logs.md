# spec-m4 — 人格展示 + debug 日志

> 依赖：M0b（可与 M1/M2 并行）。两部分独立可交付：4A 人格卡片（工具页）、4B 日志（设置页）。

## 4A. 人格卡片（工具页，只读合成视图）

### 1. Rust：`persona_snapshot`（新命令，放 `persona/mod.rs`）

```rust
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PersonaSnapshot {
    card: PersonaCardView,       // get_persona 现有内容（名字/宪法要点/表达规范/banned_forms）
    relationship_mode: String,   // relationship::current_mode → "reserved"|"familiar"|"direct"|"rusty"
    relationship_stage: i64,
    face_state: String,          // face_current
    growth_stage: i64,           // growth_status.stage
    dormant: bool,
}
```

- 纯组合命令：各取现有 State 查询函数拼装，**不改任何状态机**。
- 宪法要点展示不下发全文（persona.json 的 L1 全文偏长）→ 下发条目化数组（`constitution_items: Vec<String>`，persona 加载时按行拆好，单源在 Rust）。

### 2. 前端（工具页「人格」卡，插在感知层卡之前）

```
components/tools/PersonaCard.tsx
```

- 展示：宪法要点（折叠段）/ 表达规范（简短 · 不用 emoji 等，来自 banned_forms 摘要）/ 当前语气档（中文映射：拘谨/熟络/会顶嘴/生疏）+ 关系天数 / 当前表情 · 形态阶段。
- 只读；底注「换模型只换表达，不换本体」。不做人格编辑器（plan §5.5）。
- `relationship:stage` / `growth:stage` / `face:state` 事件到达时重拉 snapshot（useTauriEvent → invalidate）。

## 4B. debug 日志（设置页）

### 1. Rust：先接 logger（前置 bug：main.rs 从未初始化 logger，log 打点无处可去）

- 依赖：`simplelog`（轻量、零配置）。
- 初始化位置：`run()` **最前**（setup 之前，确保 setup 内日志也落盘）：

```rust
let log_dir = /* app_log_dir()，取不到回落 temp_dir */;
std::fs::create_dir_all(&log_dir)?;
let path = log_dir.join("bonsai.log");
rotate_if_big(&path, 5 * 1024 * 1024);   // >5MB → 重命名 bonsai.log.1（单代，纯函数带测）
WriteLogger::new(level, Config::default(), File::options().create(true).append(true).open(&path)?)?;
```

- 级别：debug 构建 `LevelFilter::Debug`，release `Info`。
- **已落盘文件一并纳入**：sidecar stderr 已写 `app_log_dir/llm-sidecar.log`（llm.rs serve），无需改动。

### 2. Rust：新命令（放新 `debug.rs`，≤200 行）

| 命令 | 签名 | 行为 |
|---|---|---|
| `debug_log_tail` | `file: LogFile, lines: Option<u32> → Vec<String>` | `LogFile` 枚举 `AppLog | SidecarLog`（**白名单，不接受任意路径**——防路径穿越）；默认尾部 200，上限 1000；文件缺失 → 空 Vec（不 Err，前端显示「暂无日志」）；只读倒序读（从文件尾向前找第 N 个换行，大文件不全量加载——`lines` 超过 1000 无性能风险） |
| `debug_open_log_dir` | `→ ()` | `open` 打开 app_log_dir（复用 calendar_open_settings 的 open 调用方式） |

### 3. 前端（设置页「日志」卡）

```
components/settings/LogsCard.tsx
```

- 双源 tab（应用日志 / sidecar 日志）+ 尾部 200 行等宽深色底渲染（沿用设计稿样式）+「打开目录」按钮。
- 刷新：页面可见时 10s 轮询（`document.visibilityState` 守卫，`useLogsPolling` hook）；切 tab 立即拉取。
- 日志行着色：`WARN` 黄、`ERROR` 红、`INFO` 绿（纯函数 `classifyLine(line)` 带测）。

## 4. 测试清单

Rust（cargo test，~7）：
1. `rotate_if_big`：超阈值重命名 / 未超不动 / 目标已存在覆盖（3）
2. `debug_log_tail`：尾部 N 行正确 / lines clamp 1000 / 文件缺失空 Vec / 非白名单路径拒绝（4——枚举入参使穿越不可能，测枚举边界）
3. `persona_snapshot`：R0/R1/R2 + cold 四态字段正确（并入现有 relationship 测试基建，1 组）

前端（vitest + RTL，~5）：
1. `classifyLine`：warn/error/info/未知（2）
2. `LogsCard`：tab 切换拉取 / 空文件「暂无日志」（2）
3. `PersonaCard`：模式中文映射（含 rusty）（1）

## 5. 验收清单

> 2026-11-20 真机验收通过。

- [x] `bonsai.log` 存在且 setup 期日志已落盘；>5MB 轮转生效（造大文件验证）
- [x] 设置页日志双源可切、尾部 200 行、打开目录按钮工作
- [x] 人格卡片三档语气 + cold 状态显示正确；升阶事件后卡片即时更新
- [x] release 构建（`tauri build`）日志级别为 Info

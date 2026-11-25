# 方案：应用内工作区 + 任务日记忆（App Workspace & Dated Task Memory）

| 文档信息 | 内容 |
|---|---|
| 版本 | v1（草案，待确认） |
| 日期 | 2026-11-25 |
| 状态 | **草案** — 先对齐裁决再动代码 |
| 上游 | `docs/zh/plan-files.md`（T0–T4 已实施）；`docs/zh/plan-memory.md`；`docs/zh/plan-skill-auto-extract.md`；`report.rs` / `summary.rs` 日/周报告落盘先例 |
| 定位约束 | 信任边界在 Rust；能力一份权威（`filetools` / `tasks`）；本地零工具调用；外部/用户文件正文不直写 facts（G-12）；SCHEMA 尽量不 bump |
| 一句话 | **应用自带沙箱工作区作默认读写落点 + 从审计派生「本任务读写清单」+ 任务结束引擎落盘按日 memory，SQLite 仍是权威** |

---

## 0. 一句话结论

今天 `fs_*` **默认零授权**（`tools.fs.roots=[]` → 不挂载），写文件必须用户先挑目录；任务结束只有 `tasks.final_reply` 进 SQLite，**没有**可读的「今天干了什么 / 碰了哪些文件」日文件。

本方案做三件事，且**不**另起平行 FS 实现：

1. **应用内工作区**：在 `app_data_dir()/workspace/` 建内置 root，作为模型默认读写落点（导出摘要、任务笔记、清单）。
2. **读写清单**：从既有 `tool_calls`（按 `task_id`）投影本任务 / 本日读过、写过的路径，UI 与日 memory 共用同一投影函数。
3. **按日任务 memory**：`task_finish` 后由 **Rust 引擎**追加写入 `app_data_dir()/memory/YYYY-MM-DD.md`（不是模型调 `fs_write`），便于回顾与后续总结 / skill 候选消费。

权威仍在 SQLite（`tasks` + `tool_calls`）；日 markdown 是**可浏览投影**，与 `reports/` 同族、职责不同。

---

## 1. 背景与动机

### 1.1 痛点

| 痛点 | 现状 |
|---|---|
| 「帮我写一份总结」无处可落 | roots 空时无 `fs_*`；用户未授权则无法写盘 |
| 授权心智重 | 必须系统选择器逐个加目录；对「应用自己的笔记」过重 |
| 任务事后难回顾 | `final_reply` 在 DB；无按日文件、无读写文件清单聚合 |
| skill_auto / 总结线缺可读轨迹 | 候选只投 DB；没有「今日任务 + 触达文件」的人类可读层 |

### 1.2 目标

1. **开箱可写**：总开关开时，即使未添加任何用户 root，也可在应用工作区内读写（仍过路径内核）。
2. **清单可查**：任意已结束任务能列出本次 `fs_read` / `fs_write` 路径（去重、分读写）；设置页 / 任务卡可见「今日读写」。
3. **日记忆可浏览**：成功（及可选失败）任务摘要按 `YYYY-MM-DD` 追加到本地 memory 文件，离线可打开。
4. **不削弱安全宪法**：用户授权目录语义不变；工作区不可逃出 `app_data`；delete/move/rename 仍不存在；用户文件正文仍不直写 facts。

### 1.3 非目标

| 不做 | 理由 |
|---|---|
| 把整个 `app_data`（含 sqlite、skills、密钥旁路）暴露给模型 | 最小权限；只暴露 `workspace/`（+ 只读 `memory/` 可选） |
| 默认把用户 Desktop/Documents 加进 roots | 违背 plan-files「零默认授权」 |
| 用模型 `fs_write` 写任务 memory | Ask 风暴、不可靠、可被 prompt 注入改路径；引擎写更稳 |
| 日 memory 正文直接入 `facts` | G-12；日文件是审计/回顾投影，升格走 consolidate 另议 |
| 本地模式挂 `fs_*` | AGENTS：本地零工具调用 |
| 覆盖写 / 删除工作区文件的新工具 | 沿用「新建不覆盖」；清理另开管理命令 |
| SCHEMA bump（本期） | 投影可从现有 `tool_calls.args` 抽 path；不够再开 M+ |
| 替换 `reports/` 日报 | `reports/` 仍是日历+活动日报；`memory/` 专任务轨迹 |

---

## 2. 现状盘点（代码事实）

| 事实 | 位置 | 含义 |
|---|---|---|
| `tools.fs.roots` 默认 `[]`，空则不挂 `FileToolsProvider` | `filetools.rs` / `mcp_admin::build_agent_kit` | 无「应用默认目录」 |
| 读/写同一 `resolve_in_roots` | `filetools.rs` | 新 root 必须进同一内核 |
| `fs_write` = Ask + 新建不覆盖 | plan-files §3.4 | 工作区写策略需单独裁决（见 Q2） |
| `tool_calls` 记动作/路径侧信息、不记正文 | schema + `log_agent_tool_call` | 清单可投影，无需新审计通道 |
| `task_finish` 写 `final_reply` | `memory/db/audit.rs` | 钩子点：其后追加日 memory |
| `app_data/reports/{date}.md` | `report.rs` | **引擎落盘按日文件**的先例 |
| `app_data/reports/{year}-W{week}.md` | `summary.rs` | 同族 |
| `daily_digest:YYYY-MM-DD` settings KV | `scheduler.rs` | 另一种「按日」；偏预览，非任务轨迹 |
| skill_auto 在 `task_finish(done)` 入队 | `cloud_loop` + `skill_auto.rs` | 可与日 memory **并列**挂钩，不互相替代 |
| 无 `app_data/memory/` 树 | — | 本方案新建 |

---

## 3. 架构设计

### 3.1 目录布局（应用数据）

```text
app_data_dir()/                          # Tauri app.bonsai.desktop
  bonsai.sqlite3                         # 权威：tasks / tool_calls / …
  workspace/                             # 内置 FS root（模型默认可读写）
    exports/                             # 约定：导出摘要、清单（可空，首次写时再建）
    notes/                               # 约定：短笔记（可选约定，非强制）
  memory/                                # 引擎写入的任务日记忆（建议模型只读）
    2026-11-25.md
    2026-11-26.md
  reports/                               # 既有日报/周报（不动）
  skills/                                # 既有
```

**约定**：

- `workspace/` = **工具可见、默认可写** 的沙箱。
- `memory/` = **引擎权威投影**；默认挂成 **只读 root**（或根本不进 `fs_write` 的可写集合），防止模型改写自己的审计史。
- 不把 `bonsai.sqlite3`、`skills/`、`face_skins/`、`models/` 暴露给 `fs_*`。

### 3.2 根集合拼装（挂载）

```text
build_agent_kit:
  user_roots = load_roots(conn)           # tools.fs.roots
  app_roots  = [workspace_dir]            # 若 tools.fs.enabled 或新开关
  readonly_app_roots = [memory_dir]       # 可选：仅进 search/list/read
  effective = user_roots ⊕ app_roots
  若 enabled && !effective.is_empty() → FileToolsProvider
```

相对 plan-files 的变更：

| 旧 | 新 |
|---|---|
| 仅用户 roots；空则不挂载 | 总开关开 → **至少**挂 `workspace/` |
| 模型必须猜绝对路径 | descriptor / system 附注注入 **workspace 绝对路径**（每轮构建时解析） |
| 写一律 Ask | 用户 root：仍 Ask；**工作区写策略见 Q2** |

路径内核不变：相对路径仍拒绝；模型写工作区时用注入的绝对前缀，或新增可选参数 `location: "workspace"` 由 Rust 拼路径（推荐后者，减少幻觉路径）——见 §3.4。

### 3.3 读写清单（投影，非第二套审计）

```rust
// 建议落点：filetools.rs 或 memory/db/audit.rs 旁
pub struct FsTouch {
    pub path: String,
    pub op: &'static str, // "read" | "write"
    pub ts: String,
    pub outcome: String,
}

pub fn task_fs_touches(conn, task_id) -> Vec<FsTouch>;
pub fn day_fs_touches(conn, day: NaiveDate) -> Vec<FsTouch>; // 去重可选
```

实现要点：

- 只认 `tool`/`action` 为 `fs_read` / `fs_write`（及未来同家族）且 `outcome` 成功的行。
- path 从已脱敏的 args/detail 中解析（与现审计字段对齐；解析失败则跳过并打日志，不伪造路径）。
- **禁止**把文件正文写入清单或日 memory（只路径 + 可选字节数）。

UI 消费：

- 工具页「文件」卡：保留「今天读了 N · 写了 M」，数据改走 `day_fs_touches`（今日）。
- 任务卡 / 会话侧：展开「本任务触达文件」。
- 日 memory 模板嵌入同一列表。

### 3.4 默认写入路径（模型侧契约）

推荐（Q3）：扩展 `fs_write` / `fs_read` 可选字段，**不**破坏现有绝对路径：

```json
{ "path": "exports/meeting-notes.md", "location": "workspace", "content": "..." }
```

Rust：

1. `location == "workspace"` → `workspace_dir.join(path)`，再跑文件名闸门 + `resolve_in_roots`。
2. `location` 缺省且 path 为绝对路径 → 旧行为（用户 root 或 workspace 绝对路径）。
3. `location == "memory"` → **只允许读**；写一律 `fs_memory_readonly`。

descriptor 文案明确：「默认把导出与任务笔记写到应用工作区；用户目录需已授权且用绝对路径」。

### 3.5 任务日 memory（引擎写）

**触发**：`cloud_loop` 在 `task_finish` 成功落库之后（与 `skill_auto::enqueue_candidate` 同级，互不依赖）。

**路径**：`app_data/memory/{YYYY-MM-DD}.md`（本地日历日，与 `report.rs` 一致用 `Local`）。

**追加格式（建议）**：

```markdown
## 14:32 · task_abc123 · done

- **目标**：……（goal，≤80 已裁）
- **工具调用**：3
- **读写文件**：
  - read: `/…/workspace/notes/a.md`
  - write: `/…/workspace/exports/b.md`
- **回复摘要**：（final_reply 截断 ≤500 字，超出 `…[截断]`）

---
```

规则：

- 文件不存在则建目录 + 写文件头（`# YYYY-MM-DD 任务记忆`）。
- **追加**而非整文件重写（并发：同进程 Mutex；桌面单用户可接受）。
- `done` 必写；`failed` / `canceled` 默认也写（便于复盘），可用 settings 关失败项（Q5）。
- 失败路径：**不**阻断对话返回；打结构化日志（MUST NOT silent fail 于可诊断性，但对用户主路径降级）。

**与 reports 边界**：

| | `reports/` | `memory/` |
|---|---|---|
| 生产者 | `daily_report` / 周总结命令 | `task_finish` 钩子 |
| 内容 | 日程+活动+可选 LLM 日报 | 单任务轨迹+文件清单 |
| 消费者 | 用户阅读、导出 | 用户阅读、模型只读回顾、未来总结线 |

### 3.6 数据流总览

```text
用户开 tools.fs.enabled
        │
        ▼
build_agent_kit → roots = [workspace] ∪ user_roots
        │                    (+ memory 只读可选)
        ▼
云端 tool loop：fs_read / fs_write（默认 location=workspace）
        │
        ▼
tool_calls 审计（路径，无正文）
        │
        ▼
task_finish → tasks 行更新（权威）
        │
        ├─▶ skill_auto.enqueue（既有，可选）
        └─▶ append_day_memory(day, task_id, touches, final_reply)
                 → app_data/memory/YYYY-MM-DD.md
```

```text
查询侧：
  task_fs_touches(task_id) ←── tool_calls
  日文件  ←── 引擎投影（可丢可重建；重建函数 SHOULD 提供）
```

**重建**：提供 `memory_rebuild_day(day)`（管理/测试用）从 `tasks`+`tool_calls` 重生成该日文件，保证「SQLite 权威、文件可丢」。

---

## 4. 安全与信任边界

| 条款 | 做法 |
|---|---|
| 路径权威在 Rust | 所有 I/O 仍经 `resolve_in_roots`；`location=workspace` 只是拼路径糖 |
| 最小暴露 | 仅 `workspace/`（写）与可选 `memory/`（读）；禁止向上 escape 到 app_data 根 |
| Ask 策略 | 用户目录写保持 Ask；工作区见 Q2 |
| G-12 | 用户/外部文件正文不进 facts；日 memory 只含路径与对话侧摘要 |
| 密钥 | 不把 token/DB 路径写入日 memory；敏感扩展名拒绝读规则不变 |
| 本地模式 | 不挂工具；日 memory **仍可由引擎在云端任务结束时写入**（与是否挂工具无关） |
| CSP / capability | 不新增 WebView FS；无前端直写 |

---

## 5. 与既有能力的关系

| 能力 | 关系 |
|---|---|
| plan-files | **增量**：内置 root + 可选 `location`；不推翻四不变量 |
| plan-memory 三层 | 不替代 events/facts；日文件不自动升格 |
| skill_auto | 并行消费者；日后可把「日 memory 路径列表」纳入白名单投影（**仍禁** `trace_steps.prompt`） |
| reports / summary | 同目录族不同子树；禁止混写同一文件 |
| consolidate | 本期 **不开**「把日 memory 当 event 源」；避免自噬与噪音 |

---

## 6. 分期里程碑

| 期 | 交付 | 验收 |
|---|---|---|
| **M0 工作区 root** | 启动确保 `workspace/` 存在；`build_agent_kit` 注入；descriptor 注入绝对路径；单测：空 user roots 仍挂载、越出 workspace 拒绝 | `cargo test --lib filetools::` |
| **M1 location 糖 + 清单投影** | `location=workspace`；`task_fs_touches` / `day_fs_touches`；工具页今日计数接线 | 单测 + 前端 vitest 最小 |
| **M2 日 memory 落盘** | `append_day_memory` @ `task_finish`；`memory_rebuild_day`；可选只读挂载 `memory/` | 真机：完成一任务 → 打开当日 md 见目标+文件列表 |
| **M3（可选）** | 设置页「打开工作区 / 打开今日 memory」；失败任务开关；工作区写 Ask→Auto 实验 | 产品确认后做 |

SCHEMA：M0–M2 **不 bump**。若 args 中无法稳定解析 path，再单开 M2b：`tool_calls` 增 `path` 列并 bump（需 PR 说明）。

---

## 7. 待确认裁决（Q1–Q6）

| # | 问题 | 推荐默认 | 备选 |
|---|---|---|---|
| **Q1** | 内置 root 路径名？ | `app_data/workspace/` + `app_data/memory/` | 单一 `app_data/files/` 下分 subdirectory |
| **Q2** | 工作区 `fs_write` 是否仍走 Ask？ | **仍 Ask**（与用户 root 一致，防误写刷屏）；M3 再评「workspace Auto」 | 工作区 Auto、用户 root Ask（体验更好、审计仍全记） |
| **Q3** | 默认路径如何表达？ | 可选 `location: "workspace"` + 相对子路径 | 仅注入绝对路径、靠模型拼（易幻） |
| **Q4** | `memory/` 是否对模型只读可见？ | **只读挂载**（可 `fs_read` 今日文件做回顾） | 完全不对工具暴露，仅 OS 文件管理器打开 |
| **Q5** | 哪些任务状态写入日 memory？ | `done` + `failed` + `canceled` 全写 | 仅 `done` |
| **Q6** | 日 memory 组织粒度？ | **单日单文件追加**（`YYYY-MM-DD.md`） | `YYYY-MM-DD/tasks/<id>.md`（文件多、重建重） |

实施前请闭合 Q1–Q6；未标豁免则按推荐默认。

---

## 8. 改动面预估（确认后）

| 区域 | 预估 |
|---|---|
| `filetools.rs` | 工作区路径解析、`location`、挂载拼装、只读 memory root |
| `mcp_admin.rs` `build_agent_kit` | 注入 app roots |
| `lib.rs` / 启动 | `create_dir_all(workspace)` / `memory` |
| `dialogue/cloud_loop.rs` | `task_finish` 后钩子 |
| 新模块建议 `task_memory.rs`（或 `memory/dayfile.rs`） | append + rebuild，保持 filetools 不膨胀 |
| 前端工具页 / 任务卡 | 今日读写列表、打开目录（Tauri shell open） |
| 单测 | 路径逃逸、空 user roots 挂载、append 幂等格式、rebuild 一致 |
| 文档 | 本目录为权威；`plan-files.md` 顶部加「增量见 plan-app-workspace-memory」交叉链（不覆盖历史正文） |

**不做**：新 npm/cargo 依赖；前端平行写文件；把日 memory 塞进 consolidate 主路径。

---

## 9. 质量门

| 范围 | 命令 |
|---|---|
| Rust | `pnpm check`；`cargo test --lib filetools::`；新增 `task_memory` 单测 |
| 前端 | `cd frontend && npx tsc --noEmit`；相关 vitest；改组件时 `npm run lint` |

必失败反例（测试必须红）：

1. `location=workspace` 却写入 `../bonsai.sqlite3` 或 app_data 根外。
2. 对 `memory/` 的 `fs_write` 成功。
3. 日 memory 中出现文件正文或 `trace_steps.prompt`。
4. `tools.fs.enabled=0` 时仍挂载 app workspace 工具。
5. 仅删 markdown 后 `rebuild` 与 DB 投影不一致（路径集合）。

---

## 10. 风险与缓解

| 风险 | 缓解 |
|---|---|
| 工作区变成「第二个垃圾抽屉」 | 约定子目录 + 设置页「打开/揭示」；不提供删工具，清理靠用户或日后管理命令 |
| Ask 每次写工作区打断心流 | Q2 推荐先 Ask；数据证明噪声后再开 workspace Auto |
| 日文件膨胀 | 单条回复截断 500；仅路径；可选保留最近 N 天（M3） |
| 与 skill_auto 双写钩子失败互相影响 | 两钩子独立 try；任一失败只打日志 |
| 模型忽略 `location` 瞎写绝对路径 | descriptor + 注入路径；错误码明确提示 |

---

## 11. 变更记录

| 版本 | 日期 | 说明 |
|---|---|---|
| v1 | 2026-11-25 | 初稿：工作区默认落点 + 读写清单投影 + 按日任务 memory；Q1–Q6 待确认 |

# 总结 skill：日报 / 周总结（plan-summary-skill）

> 2026-11-22 落地。目标：让阿盆能回答「总结一下今天」「这周都干了啥」，并给出两份可归档的总结。
> 本文记录裁决依据、实现边界与未做项，供后续接手。

## 1. 需求与现状核查

需求：新增一个「总结」能力，拆成两个子 skill —— 日报总结与周总结。

动手前核查了现有代码，三个事实决定了方案：

| 事实 | 位置 | 影响 |
| --- | --- | --- |
| skill 机制已存在，但是 **prompt-only**：目录 `<data>/skills/<id>/SKILL.md`，正文 ≤8KB | `src-tauri/src/skill.rs` | 纯 SKILL.md 没有取数能力，拿不到总结素材 |
| skill 段**只在云端路径注入** | `dialogue.rs::cloud_tool_loop` → `skill_section()` | 默认本地 qwen3.5:2b 下纯 skill 包完全不生效 |
| 日报底座已存在：`DayDigest`（剪贴板/下载/应用/新记忆）+ `daily_report` | `src-tauri/src/report.rs` | 感知类日报不必重写，可直接复用查询思路 |
| 同时注入上限 `SIMULTANEOUS_SKILLS = 2`，且是「按排序取前 2 个」 | `skill.rs` | 两个总结包一旦启用就占满配额，会挤掉用户自己的 skill |

## 2. 裁决

**双层：引擎 + 两个 skill 包；对话与按钮双触发。**（2026-11-22 用户确认）

- **引擎层 `summary.rs`**：规则路由意图 + 窗口素材聚合 + prompt 组装 + 确定性回落。事实全部由 Rust 决定，模型只写散文。
- **skill 层两个子包**：只做输出契约（结构 / 长度 / 口吻 / 不编造）。命中意图时定向注入，**不进**通用 `skill_section`，因此不占 `SIMULTANEOUS_SKILLS` 配额。
- 双触发：对话里说「总结一下今天」走规则路由；工具页「本周总结」按钮走 `weekly_report` 命令，同一套素材。

## 3. 设计

### 3.1 意图路由（`summary::intent`）

模型无关，纯规则。**必须同时**命中「总结动词」与「时间词」，否则 miss：

- 动词：总结 / 回顾 / 复盘 / 汇总 / 梳理 / 干了啥 / 干了什么 / 做了什么 / 进展 / 日报 / 周报
- 周词：本周 / 这周 / 这一周 / 这礼拜 / 最近7天 / 一周 / 周报
- 日词：今天 / 今日 / 今儿 / 日报 / 这一天
- 排除对象（先判）：纪要 / 记录 / 模板 / 怎么写 / 这段代码 / 这个方案 / 会议内容 …

周词优先于日词（「今天和这周」→ 周总结，宽窗口包含窄窗口）。
与 `tools.rs` 的日历路由不冲突：那边 `EXCLUSIONS` 已含「总结」，日历会主动让路。

### 3.2 窗口

| 类型 | 窗口 | 说明 |
| --- | --- | --- |
| 日报 | 今天 00:00 → 此刻 | 「今天」指到现在，不是整天 |
| 周总结 | 本周一 00:00 → 此刻 | 中文「这周」= 本周，不是「最近 7 天」 |

Iso 串供事件表（`events`/`tool_calls`），日期串供日键表（`predictions`/`relationship_progress`），两者同源同一跨度。

### 3.3 素材（`Digest`）

| 字段 | 来源 | 上限 |
| --- | --- | --- |
| `turns` | `events` 中 `kind=dialogue, role=user` | 日报扫 60 / 周报扫 300，展示 6 / 12 |
| `facts` | `facts` 窗口内 | 展示 6 / 10 |
| `perceived/clipboards/downloads/apps` | `events` 中 `perception` 前缀 | 应用 Top 3 |
| `stats.tools` | `tool_calls` 窗口内 `tool:action` 计数 | Top 5 |
| `stats.predictions_*` | `predictions`（`asked_at` 非空才算「问过」） | — |
| `stats.relationship_*` | `relationship_progress` 累计分与天数 | — |

两条纪律：

1. **素材不含内容型泄漏**：`window_stats` 只回计数，不带日程标题、工具参数。
2. **意图句自身不进素材**：`总结一下今天` 这句话不会被当成「今天发生的事」。

### 3.4 注入与回落

- system = 人格 system（对话路径）+ `【总结模式】` + `【输出契约（untrusted）】`
- user = 时间范围 + 素材块 + 一句输出要求（2B 模型对短 user turn 的服从度远高于长 system 前言）
- 素材为空 → **不调用模型**，直接给诚实空答（没有事实可依据的总结只能是编造）
- 模型返回空（挂了）→ `fallback()` 的确定性文本：只报计数，不加修饰
- 流式路径下回落文本会补发一次 delta，避免空气泡

总结的 `filter` 报告照记，但**不写 disagreement 事件**——那是关系线「顶嘴」的证据，总结里出现「不同意」字样不该算数。

### 3.5 skill 包的种子与生命周期

- 正文编译进二进制（`include_str!`），首次启动种入 `<data>/skills/{daily-summary,weekly-summary}/SKILL.md`
- 用 setting `summary_skills_seeded` 保证**只种一次**：之后用户改过就保留、删掉就不复活
- 引擎读盘优先，包缺失时回落到编译副本 → 删包不会坏功能，只会丢自定义契约
- 两个包在设置页可见、可启停、可删；但它们不进通用注入，启停只影响「是否用自定义契约」这一层语义（当前实现：引擎始终读盘，启用状态不参与判断）

## 4. 实施记录

| 文件 | 改动 |
| --- | --- |
| `src-tauri/src/summary.rs`（新） | 意图路由 / 窗口 / 素材聚合 / prompt / 回落 / markdown 渲染 / `weekly_report` 命令 |
| `src-tauri/src/skill.rs` | `ENGINE_PACK_IDS`、`is_engine_pack`、`load_engine_packs`、`builtin_body`、`seed_builtin_packs`（幂等、不覆盖） |
| `src-tauri/src/memory/db.rs` | `WindowStats` + `window_stats`（tool/预测/关系三路计数） |
| `src-tauri/src/mcp_admin.rs` | `AgentKit.engine_skills`；通用 skill 注入过滤引擎包 |
| `src-tauri/src/dialogue.rs` | 日历路由之后的总结分支 + `summary_turn`（锁内聚合、锁外推理、照常落库与 trace） |
| `src-tauri/src/lib.rs` | `mod summary`、`weekly_report` 注册、启动种子 |
| `src-tauri/resources/skills/*/SKILL.md` | 两个子 skill 的输出契约 |
| `frontend/src/lib/commands.ts` | `weeklyReport()` |
| `frontend/src/components/tools/ActionRow.tsx` | 「本周总结」按钮，复用 `onReport` 弹层 |

门禁：`cargo test` 355 通过（新增 12 个：意图 4 / 窗口 2 / 素材 4 / prompt 1 / skill 种子 3 / db 统计 1 / 对话路由 3）；`cargo clippy` 无新增警告；前端 `tsc --noEmit` 干净、`vitest` 94 通过。

`AgentKit` 加字段后的 15 处测试构造点统一用 `..crate::mcp_admin::AgentKit::empty()` 收尾，避免逐处补字段。

## 5. 未做 / 后续

| 项 | 原因 | 建议 |
| --- | --- | --- |
| 「上周总结」 | 窗口要回退一周，本轮不做 | 加 `SummaryKind::LastWeek`，复用 `window()` |
| 日程素材进总结 | 需接 `CalendarSource`（TCC + 主线程），且日报已有 `TomorrowAgenda` | 复用 `report::tomorrow_agenda`，素材块加一行 |
| 归档页（历史总结可回看） | 需要 schema v13 与前端新路由，本轮判断为超范围 | 现在已按 `reports/2026-W48.md` 落盘，够手工回看 |
| 真机验收 | 本地 2B 成文质量、周报在真实 7 天数据上的表现 | `cargo tauri dev` 后说「总结一下今天」「这周总结一下」各一次，再看 `/trace` |

## 真机修复：具体日期开窗（f718b56，2026-11-22）

真机 trace 暴露的问题：用户说「生成 20 日的日报」，意图命中 Daily，但窗口只支持
今天/本周，`20日` 被忽略 → 按今天开窗，素材全错；且总结路由刻意不叫工具
（素材由 Rust 聚合），模型无从自救，也没有任何工具调用痕迹。

修复：`resolve_window()` 复用 `memory_search::parse_window` 的日期解析——
裸 D 日/M月D日/昨天/周X/N天前/整周短语全部生效，命中即按那个日期开窗
（纯过去窗口上限 = 次日零点，不偷跑后面几天的事件）；没命中回落今天/本周默认。
`collect` 拆出 `collect_in(conn, kind, window)`，对话路由先解析再聚合。
`WEEK_WORDS` 补 `上周/上上周`（配合 parse_window 的整周窗口）。

新增测试：`a_pinned_date_beats_the_default_window`、`pinned_weeks_and_default_fallback`。

# 方案：三层记忆 + 夜间整理（MVP P0 第 3 项）

> **状态**：M1–M4 已实施（2026-11-18，38 单测全过）；M5（LocalExtractor，本地 LLM）挂起等本地模型接入
> **日期**：2026-11-18
> **依赖**：本地 LLM 接入（Ollama 兼容 API，模型调用全在本机；未接入前走规则 fallback）
> **对应调研**：docs/zh/research-report.md §4.1–4.4

---

## 1. 目标与非目标

### 目标

1. 落地三层记忆结构：**事件层（原始）→ 事实层（提炼）→ 偏好/边界层（高阶）**，
   每一层向上可追溯（可解释）、向下可删除（可遗忘）。
2. 落地夜间 consolidate job 完整五阶段：**压缩 → 抽取 → 合并 → 矛盾消解 → 删除**。
3. 建立调度器：每晚 03:00 自动跑，错过可补跑，幂等。
4. 检索升级为真 hybrid：keyword（已有）+ 时间衰减（P0 内）+ 向量（P0 后，见 §9）。
5. 建立三个一等评估指标：记忆命中率、误记率、衰减正确率（§8）。

### 非目标（P0 明确不做）

- 不做跨设备同步（P2）。
- 不做 Live2D/情绪脸联动（face.rs 已独立实现）。
- 不做向量语义检索的最终版（先留 schema 与接口位，P0 用 `TODO(vector)` 占位）。
- 不做多用户/多宠物档案隔离。

---

## 2. 现状盘点（写方案前的代码事实）

| 已有 | 位置 | 说明 |
| --- | --- | --- |
| 4 张表 schema | `memory/db.rs` | events / facts / preferences / boundaries；WAL + FK |
| 事件/事实写入 | `db::add_event` / `db::add_fact` | ts 为 RFC3339 UTC |
| 软失效 | `db::invalidate_fact` | 打 `invalid_at`，不物理删 |
| 硬删除（合规） | `db::forget_event` | 级联删该事件产出的所有 facts |
| keyword 检索 | `db::search` | LIKE；facts 优先、events 补位 |
| 精确去重 | `consolidate::run` | 仅 `GROUP BY content` 精确匹配合并 |
| Tauri 命令 | `memory/mod.rs` | add_event / add_fact / search / consolidate |

| 缺口 | 说明 |
| --- | --- |
| 事实抽取 | 没有任何「从当天对话抽出事实」的通路 |
| 偏好四分类 | preferences 表有 category 字段但从未写入分类逻辑 |
| 矛盾消解 | 只能手动 invalidate，无自动判定 |
| 低价值删除 | 完全没有 |
| 调度器 | consolidate 只能前端手动触发命令 |
| 时间衰减 | 检索排序无 decay |
| 事件来源标注 | 对话事件没有 role（user/assistant）与 session 概念 |

---

## 3. 三层记忆的精确定义

```text
┌──────────────────────────────────────────────────────────────┐
│ 第三层  偏好/边界层   高置信、长期稳定的用户模型              │
│         preferences(category ∈ 四类) + boundaries            │
│         只能由 consolidate 从 facts 升格产生，置信度显式       │
├──────────────────────────────────────────────────────────────┤
│ 第二层  事实层        从对话提炼的单条可断言知识              │
│         facts：一句话一个事实，带 source_event_id 溯源        │
│         被推翻时打 invalid_at，永不静默修改                   │
├──────────────────────────────────────────────────────────────┤
│ 第一层  事件层        原始记录，唯一 ground truth             │
│         events：对话轮次、用户操作、感知事件；只增不改         │
└──────────────────────────────────────────────────────────────┘
```

**三条不变式**（全部代码强制，不是约定）：

1. **溯源不变式**：facts 与 preferences 必须能追到至少一个 event；
   追不到来源的条目视为脏数据，consolidate 时清除。
2. **不静默改写不变式**：事实被推翻只能 `invalid_at`，旧值永不 UPDATE；
   「用户说忘掉」才走硬删（`forget_*`，级联到所有派生层）。
3. **本地不变式**：三层全部落本地 SQLite；模型推理（对话/抽取/消解）**优先本地 LLM**
   （Ollama 兼容 API），对话与记忆数据不出机器。若用户显式启用云端增强，云端只接收
   *为了抽取而临时必需的片段*，返回的是结构化事实；离线或隐私模式下抽取降级为规则版。

**偏好四分类**（调研 §4.1，category 枚举）：

| category | 含义 | 例 |
| --- | --- | --- |
| `like` | 显式偏好 | 「喜欢雨天」 |
| `habit` | 行为规律 | 「早上 10 点才开工」 |
| `context` | 稳定背景 | 「在用 Rust 写桌面项目」 |
| `boundary` | 不许做 | 「别在我专注时弹窗」→ 同时写 boundaries 表 |

---

## 4. 数据模型变更（迁移 v1 → v2）

```sql
-- 事件层：补对话元数据
ALTER TABLE events ADD COLUMN role TEXT;            -- 'user' | 'assistant' | 'system'，NULL=非对话事件
ALTER TABLE events ADD COLUMN session_id TEXT;      -- 同一次对话窗口内同 id
ALTER TABLE events ADD COLUMN consumed INTEGER NOT NULL DEFAULT 0;  -- 已被某次 consolidate 消费

-- 事实层：补置信度与重要性
ALTER TABLE facts ADD COLUMN confidence REAL NOT NULL DEFAULT 0.6;
ALTER TABLE facts ADD COLUMN importance INTEGER NOT NULL DEFAULT 3; -- 1..5，删除判据之一

-- 偏好层：category 收紧为四类枚举 + 出现次数（置信度随重复出现上升）
-- boundaries 增加来源事实
ALTER TABLE boundaries ADD COLUMN source_fact_id INTEGER REFERENCES facts(id);
```

迁移器：`db::migrate(conn)`，按 `PRAGMA user_version` 判断版本，v1 库自动升级；
`PRAGMA user_version` 置 2。测试覆盖「v1 旧库打开 → 升级 → 旧数据无损」。

**新增表（consolidate 审计）**：

```sql
CREATE TABLE IF NOT EXISTS consolidate_runs (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    started_at    TEXT NOT NULL,
    finished_at   TEXT,
    trigger       TEXT NOT NULL,   -- 'nightly' | 'manual' | 'catchup'
    window_start  TEXT NOT NULL,   -- 消费的事件窗口 [start, end)
    window_end    TEXT NOT NULL,
    extracted     INTEGER NOT NULL DEFAULT 0,
    merged        INTEGER NOT NULL DEFAULT 0,
    contradicted  INTEGER NOT NULL DEFAULT 0,
    deleted       INTEGER NOT NULL DEFAULT 0,
    status        TEXT NOT NULL DEFAULT 'running'  -- 'running'|'done'|'failed'
);
```

---

## 5. 夜间整理管线（consolidate v2）

### 5.1 流程

```text
取窗口内未消费 events（默认 [昨日00:00, 今日00:00)，catchup 时前推）
  │
  ├─ ① 压缩    同 session 的对话轮 → 「当天会话纪要」1..3 条（写回 events, kind='summary'）
  ├─ ② 抽取    纪要 + 原始轮次 → 候选事实（LLM；无模型时规则版，见 5.3）
  ├─ ③ 合并    候选 vs 存量 active facts：精确去重（已有）+ 归一化相似合并
  │            重复出现 → 原事实 confidence += 0.1（上限 0.95）并记出现次数
  ├─ ④ 矛盾消解  候选与存量语义对立（如喜欢/讨厌同一对象）→ 旧事实 invalidate，
  │            新事实 source 链回旧事实 id，保留「曾经以为」的可解释历史
  ├─ ⑤ 升格/删除  confidence ≥ 0.8 的事实升格为 preference（带 category）
  │            importance ≤ 1 且 confidence < 0.4 且 30 天未被检索命中的 facts → invalidate
  └─ 收尾      events.consumed = 1（窗口内）；写 consolidate_runs 审计行
```

### 5.2 模型适配层（关键解耦点）

```rust
// memory/extract.rs
pub trait FactExtractor: Send {
    /// 输入当天会话片段，输出候选事实 + 建议分类 + 对立检测提示。
    fn extract(&self, session: &SessionDigest) -> Vec<CandidateFact>;
}

pub struct RuleExtractor;       // 无模型/离线兜底：确定性、零依赖
pub struct LocalExtractor { … } // 默认模型腿：本地 LLM（Ollama 兼容 API），同一 trait
```

- `LocalExtractor` 走本地 LLM（Ollama 兼容 OpenAI API，默认 `localhost:11434`；
  人格卡注入 → 模型 → 结构化 JSON），prompt 要求返回 JSON 数组：
  `{fact, category?, contradicts_fact_id?, confidence}`。对话与记忆原文不出机器。
- 云端 API 仅作为**可选**质量增强（用户显式开启，同一 trait 换实现即可），不在 P0 范围。
- 模型失败/超时（10s）→ 自动降级 `RuleExtractor`，审计行记录 `status='failed'` 但不清 consumed，
  下次 catchup 重试，保证**不丢事件**。

### 5.3 RuleExtractor（P0 兜底规则）

| 规则 | 触发模式 | 产出 |
| --- | --- | --- |
| 直接陈述 | 「我(喜欢/讨厌/在用/每天…)X」 | fact + category 推断 |
| 更正语 | 「不对/说错了/其实是 X」 | 最近同主题事实 invalidate |
| 显式边界 | 「别/不要再 X」 | boundaries 直写（最高优先级，不经 confidence） |
| 重复提及 | 与存量 fact 内容归一化后相同 | confidence 上升 |

归一化：去标点 + 小写 + 去空白；后续向量上线后替换为语义相似。

### 5.4 幂等与并发

- consolidate 在**独立线程**跑，持锁 `MemoryStore` 互斥量；命令入口立即返回 run id，前端轮询审计表。
- 每次运行以 `consolidate_runs.window_start/end` + `events.consumed` 双重保证不重复消费。
- 单事务包裹 ②–⑤；失败整体回滚，事件保持未消费。

---

## 6. 调度器（`memory/scheduler.rs`，新文件）

```rust
pub fn spawn(app: tauri::AppHandle) {
    // std::thread + 循环：每 60s 检查一次本地时间
    // 触发条件（三选一满足即跑，跑完记录 last_run_date 防重）：
    //   1. 首次跨过 03:00（当日未跑过）
    //   2. 启动时发现 last_run_date < 今天 且 现在时间 ≥ 03:00 → catchup（补昨晚）
    //   3. 启动时距上次运行 > 36h → catchup
}
```

- 不引额外调度 crate，一个 60s 轮询线程足够 MVP。
- 用 **本地时区**判断「03:00」与「昨天窗口」（用户生活节奏是本地的；表内存储仍 UTC）。
- 设置项（后续接前端）：`consolidate.enabled`（默认 true）、`consolidate.hour`（默认 3），
  先以 `resources/app_config.json` 占位。
- 完成后向前端发 `consolidate:done` 事件（载荷 = 审计行），
  给「它昨晚想了想你说的话」的成长叙事留钩子。

---

## 7. 检索升级（P0 范围内）

在现有 keyword 腿上加**时间衰减排序**，一次 SQL 改造：

```sql
ORDER BY (invalid_at IS NULL) DESC,
         score DESC
-- score = 关键词命中(1) * importance/3 * exp(-days_since_ts / 45) * (0.5 + 0.5*confidence)
-- 在 Rust 侧计算 score 后排序（避免 SQLite 数学函数兼容性问题）
```

- 半衰期 45 天：超过一个半月的记忆自然下沉，boundary 与 preference 不衰减（常驻）。
- 检索命令加 `with_decayed: Option<bool>`，成长叙事需要「翻旧账」时可关。
- 向量腿不在 P0：`db.rs` 中保留 `TODO(vector)`，schema 预留 `facts.embedding BLOB`
  **不加列**，等选型（sqlite-vec vs 存 JSON）定了再迁移，避免双倍迁移。

### 7.1 索引层（S2，2026-11-23 落地）

关键词腿的候选集改由 `memory/index.rs` 提供，不再对四张表各做一次
`LIKE '%q%'` 全扫。打分公式（上面那条）一个字没改——换的是「谁能进候选」，
不是「候选怎么排」，因此 §8 的三项指标保持可比。

- **不是 trigram**：`trigram` 分词器对少于 3 个字符的查询返回 0 行，而中文
  的检索单位就是 2 字词（`报销`/`咖啡`/`窗户` 全部零命中）。实测结论固化
  在 `index::tests::bundled_sqlite_has_fts5_and_trigram_cannot_do_two_chars`。
- **做法**：正文 → 空格分隔的 2-gram（ASCII 段整词）→ `search_docs`
  （普通表，`UNIQUE(kind, ref_id)`）→ `search_fts`（FTS5 `unicode61`，
  外部内容指向 `search_docs`）。索引侧与查询侧共用 `tokens()`，对称即正确。
- **维护**：新增走水位线增量同步（`search_sync.last_id`，包在 SAVEPOINT 里
  避免逐条 fsync）；删除由源表 AFTER DELETE 触发器级联（用户「忘掉」必须
  连索引一起忘，不依赖应用层记得调）。
- **跨时间召回**：默认窗口是最近 7 天，三个月前的事实原先没有通路。
  `memory_search::recall` 在**用户没指定日期**时追加 `beyond` 腿：有内容词走
  索引按相关度取窗口前的事实/偏好/边界；无内容词（「还记得吗」）取
  importance × confidence × decay 最高的几条。基线里「久远事实」0% → 100%。

---

## 8. 评估（P0 交付物之一，不是 P1）

新建 `memory/eval.rs` + `benches/memory_eval.rs` 形式的测试集：

| 指标 | 方法 | 目标 |
| --- | --- | --- |
| 记忆命中率 | 固化 50 条种子事实 → 50 个依赖问题 → 检索 top-k 是否含正确事实 | > 85% |
| 误记率 | 检索结果中已 invalidate 仍被引用 / 归一化后错误事实占比 | < 1% |
| 衰减正确率 | 注入 30 条过期事实（invalid_at 或低分衰减），问题不依赖它们 | 100% 不引用 |

评估用**规则版抽取器 + 固定时钟**（`now()` 改为可注入，避免测试依赖真实时间）。
三项指标以测试形式常驻 CI，抽取向量/规则改动必须过这套。

---

## 9. 里程碑拆分（对齐 8–10 周 MVP 中的本项工作量）

| 里程碑 | 内容 | 验收 |
| --- | --- | --- |
| M1（0.5 周） | schema v2 迁移 + role/session 元数据 + 审计表 | `cargo test` 过；v1→v2 迁移测试过 |
| M2（1 周） | RuleExtractor + 管线 ②③④⑤ + 事务幂等 | 种子对话集跑出：抽取正确率 ≥80%（规则可覆盖的句式内）；重复消费不产生重复事实 |
| M3（0.5 周） | 调度器线程 + catchup + `consolidate:done` 事件 | 伪造系统时间跨 03:00 自动触发；杀进程重启后补跑不丢不重 |
| M4（0.5 周） | 时间衰减检索 + eval.rs 三指标 | 三指标达标（命中率>85%、误记率<1%、衰减正确率 100%） |
| M5（本地模型接入后） | LocalExtractor 实现（本地 LLM）+ 降级验证 | 与 RuleExtractor 跑同一评估集对比，遗漏下降 ≥30%（对照调研数字） |

M1–M4 不依赖模型层，可立即开工；M5 挂起等本地 LLM 接入。
`pnpm app` 实测（P0 阻塞项之一）时顺带验证：手动触发 consolidate → 前端收到 done 事件。

---

## 10. 风险与开放问题

| 风险 | 应对 |
| --- | --- |
| 规则抽取中文分词弱（「我讨厌挤地铁」切不准） | P0 只匹配固定句式前缀；M5 用模型兜底 |
| 矛盾消解误判（「喜欢雨天」vs「讨厌今天的雨」不是矛盾） | 规则版只处理「同对象+反义动词」的强模式，宁漏勿误；语义矛盾留给模型 |
| consolidate 与用户对话并发写库 | WAL + Mutex 已有；写入全走同一 Connection，线程阻塞窗口 < 1s |
| 硬删的级联范围 | `forget_event` 需扩展为 `forget_session` / `forget_all`（合规第 16 条），随 M1 一起做 |
| 时间衰减把 boundary 误沉 | boundary/preference 明确豁免衰减，写入单测 |

**开放问题（评审时定）**：
1. 会话纪要（阶段①）是否在 P0 保留？无模型时纪要质量存疑，可先只做 ②–⑤。
2. `importance` 初值由谁定：写入时规则估（M2）还是全部默认 3（更简）？
   建议 P0 全默认 3，importance 留字段不留逻辑。

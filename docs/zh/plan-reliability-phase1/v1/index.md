# 方案：Phase 1 Reliability（Forget / 失效过滤 / 同义分型 / 主动配额）

| 文档信息 | 内容 |
|---|---|
| 版本 | v1 |
| 日期 | 2026-11-24 |
| 状态 | **已实施**（2026-11-24；按 §9 推荐默认落地；`cargo test --lib` 628 绿） |
| 上游输入 | [`roadmap-architecture-r2/v1`](../../roadmap-architecture-r2/v1/index.md) §4；第二轮架构 Review；代码锚点 `memory/db/events.rs` / `synonym.rs` / `decision.rs` / `memory_search.rs` |
| 定位约束 | 本地优先与隐私承诺；能力一份权威；变更最小化；**不做** Vector / Durable Agent / 隐式记忆注入（属 Phase 2） |
| 一句话 | **先忘掉干净、召回不掺假、配额跨重启诚实** |

---

## 0. 一句话结论

本阶段只修四条可靠性链路，不扩产品能力面：

1. **R1** 统一 Memory Provenance + Forget Service，让 Session/Event 删除真正级联到 Preference / Boundary / Profile（有多源证据则保留）。
2. **R2** 默认 Recall 路径排除已失效 Fact；历史回顾另标，不混入「当前事实」。
3. **R3** 同义簇改为带类型的语义关系；对立 / 因果禁止直接扩腿。
4. **R4** 主动消息日配额落 SQLite、统一本地日界；成功发出才消耗。

**R5** 不是独立功能，是上述项的回归测试最低集（M06/M07/M08/M10），与 R1/R2 同门禁。

执行序：**R1+R5 → R2（可同 PR）→ R4（独立 PR）→ R3（独立 PR）**。

---

## 1. 目标与非目标

### 1.1 目标

| ID | 用户可感知结果 |
|---|---|
| R1 | 「忘掉这段对话」后，系统不能再通过长期记忆「记得」该会话独有信息 |
| R2 | 已被纠正的旧事实，不会在普通回忆里装成当前事实 |
| R3 | 「不加糖」不会因为同义表召回「加糖」类记忆 |
| R4 | 同一本地自然日、含重启，主动搭话不超过产品承诺次数 |

### 1.2 非目标（本方案明确不做）

| 不做 | 归属 |
|---|---|
| 隐式相关记忆自动注入 Context | Phase 2 · I1/I2 |
| Consolidation Evidence Independence / Correction 大改 | Phase 2 · I3（R1 只要求**写入** evidence，不改置信公式） |
| Vector / Embedding Hybrid | 能力 roadmap P5-1 |
| Task 崩溃自动续跑 / 步骤幂等 | Phase 3 · A1/A2 |
| Decision → Action Ranking 引擎 | Phase 3 · A4 |
| 为拆文件而重构 `mcp_admin` / `consolidate` | 仅在本方案触及文件时顺手切开 Forget |
| 环境段改纯 tool 化 | 另议；ambient 时间继续固定注入 |

---

## 2. 现状盘点（写方案前的代码事实）

| 事实 | 位置 | 对本方案的意义 |
|---|---|---|
| `SCHEMA_VERSION = 17` | `memory/db/schema.rs` | 本方案 bump → **18** |
| `forget_session` 只删 trace / tasks / facts / sessions / events | `events.rs` ~192–205 | **漏** preferences、boundaries、user_profile、user_profile_history |
| `forget_event` 只删 facts + event | `events.rs` ~179–182 | 同样不级联 L3 / Profile |
| `forget_all` 已清 preferences / boundaries / profile | `events.rs` ~212–216 | All 路径相对完整；Session/Event 才是洞 |
| `preferences` **无** `source_fact_id` / `source_event_id` | schema `preferences` | 今日无法按会话反向清理；必须上 Provenance |
| `boundaries.source_fact_id` 可选 | schema | 可走 Fact→Event，但仍缺统一多源模型 |
| `user_profile.source_event_id` 可选；`source='manual'` 不被自动覆盖 | schema + `profile.rs` | Forget 时 **manual 永不因会话删除而清** |
| FTS：源表 DELETE → trigger 删 `search_docs` → FTS | schema 触发器 | Forget 物理删行即可；**软失效**不会自动出索引 |
| `db::search` 对 invalidated 乘 0.1，不排除 | `events.rs` scoring | 仍可能进候选 |
| `beyond_window` 不读 `hit.invalidated` | `memory_search.rs` ~617–633 | **P1 泄漏点**（审查已核实） |
| 同义簇含对立/上下位/因果混用 | `synonym.rs` `CLUSTERS` | 如「蔗糖/加糖/不加糖」「花生/坚果」「过敏/不能吃」 |
| `should_speak` 用 `Utc::now().date_naive()`，配额在内存 `ProactivityState` | `decision.rs` | 与 `evaluate_live` 的 Local 日界不一致；重启重置配额 |
| Speak 在 `should_speak` 内先 `used_today += 1`，再异步生成/emit | `decision.rs` + `proactive.rs` | 失败仍可能耗配额；或并发双 Speak |
| 现有 `forget_session_and_all_cascade` **未断言** preference/boundary/profile | `db/tests.rs` | 回归缺口；M06 需重写加强 |

---

## 3. 不变量（违反即回滚）

| 编号 | 不变量 |
|---|---|
| I1 | **可遗忘**：用户发起的 Session/Event 删除后，**仅由该来源支撑**的派生记忆不得残留（含 FTS） |
| I2 | **多源保留**：一条 Preference/Boundary/Profile 若仍有其他有效 Evidence，删除其中一个 Session/Event **不得**抹掉整条 |
| I3 | **manual Profile 豁免**：`user_profile.source = 'manual'` 不因 Forget Session/Event 自动删除 |
| I4 | **不静默改写**：Forget 是硬删或摘证据；Fact 被纠正仍走 `invalid_at`，本方案不改该语义 |
| I5 | **单点 Forget**：Session/Event/All 的级联逻辑只住在 Forget Service；`events.rs` 薄封装或删除平行 DELETE 列表 |
| I6 | **本地日界**：主动配额与「每日一次」文案一律 `clock::now_local().date_naive()`（可 pin） |
| I7 | **成功才计次**：配额只在主动消息**成功发出**（或明确产品定义的成功点）后提交；Gate 通过 ≠ 已消耗 |
| I8 | **本地零工具不变**：本方案不新增对话工具调用路径 |
| I9 | **孤儿诚实**：无 Evidence 的历史 Preference 在 Forget Session 时**不假装已溯源删除**（见 §4.5） |

---

## 4. R1 — Memory Provenance + Forget Service（P0）

### 4.1 数据模型

新增表（schema v18）：

```sql
CREATE TABLE IF NOT EXISTS memory_evidence (
    memory_type TEXT NOT NULL,   -- 'fact' | 'preference' | 'boundary' | 'profile'
    memory_id   TEXT NOT NULL,   -- fact/pref/boundary 用十进制 id；profile 用 key
    event_id    INTEGER NOT NULL REFERENCES events(id) ON DELETE CASCADE,
    relation    TEXT NOT NULL,   -- 'extracted' | 'promoted' | 'derived' | 'manual_link'
    created_at  TEXT NOT NULL,
    PRIMARY KEY (memory_type, memory_id, event_id, relation)
);
CREATE INDEX IF NOT EXISTS idx_evidence_event
    ON memory_evidence(event_id);
CREATE INDEX IF NOT EXISTS idx_evidence_memory
    ON memory_evidence(memory_type, memory_id);
```

说明：

- `memory_id` 用 TEXT，避免 profile key 与整型 id 分裂两套表。
- `ON DELETE CASCADE` 挂在 `events`：删 Event 时证据行先掉；**派生记忆是否删**由 Forget Service 在同事务内根据「是否还有其他 evidence」决定（见 §4.3）。注意：若仅靠 FK CASCADE 删 evidence、却忘了删无证据记忆，会留下孤儿——所以 **Service 必须在删 Event 之前**按 event_id 查出派生集并裁决。
- `relation` 先枚举四值即可；Phase 2 的 EvidenceType 可扩展，本阶段不引入 independence_group。

**不**给 `preferences` 加 `source_fact_id` 列作为权威——权威只在 `memory_evidence`，避免平行溯源。

### 4.2 写入点（谁负责挂 Evidence）

| 写入动作 | 现有函数 | 变更 |
|---|---|---|
| 抽取 Fact | `add_fact` / `add_fact_confident` | 若有 `source_event_id`，同事务 `evidence(fact, id, event, extracted)` |
| 升格 Preference | `promote_fact` / `add_preference` | **必须**传入至少一个 `event_id`（从 fact 的 source 或调用方显式传入）；写 `evidence(preference, …, promoted)` |
| Boundary | `add_boundary` | 若有 `source_fact_id`，解析 fact→event 后写 `evidence(boundary, …, derived)`；无 fact 则禁止无源写入（测试环境可显式 `manual_link`） |
| Profile upsert | `profile` 写入路径 | 非 manual 且有 `source_event_id` 时写 `evidence(profile, key, event, extracted)`；manual 可写 `manual_link` 或不写（Forget 靠 I3） |

Consolidate 升格路径今日调用 `promote_fact(conn, category, content, confidence)` **无 event**——签名扩展为：

```rust
pub fn promote_fact(
    conn: &Connection,
    category: &str,
    content: &str,
    confidence: f64,
    source_event_ids: &[i64],  // 至少 1 个；空切片 → Err，禁止无源升格
) -> rusqlite::Result<bool>;
```

同 content 已存在 active preference 时：不新建行，但 **合并补写**缺失的 evidence（支撑 M08）。

### 4.3 Forget Service 算法

新模块：`src-tauri/src/memory/forget.rs`（权威实现）。

```text
forget_events(conn, event_ids: &[i64])  // 同事务
  1. 锁定：BEGIN IMMEDIATE（或外层已有写事务则复用）
  2. 查询 memory_evidence WHERE event_id IN (…)
     → 得到待裁决集合 Map<(type, memory_id), remaining_other_evidence?>
  3. 对每个派生项：
       - 删除「本批 event」对应的 evidence 行
       - 若该 memory 不再有任何 evidence：
           · fact → DELETE fact（触发器清 FTS）
           · preference → DELETE preference
           · boundary → DELETE boundary
           · profile → 若当前行 source!='manual' 则删 key，并写入 user_profile_history；
                       manual → 只摘 evidence，保留 KV
  4. 仍按今日逻辑清理：该 event 上「仅靠 source_event_id、尚无 evidence 行」的遗留 facts
     （兼容迁移窗口；evidence 齐全后此兜底变为空操作）
  5. DELETE events WHERE id IN (…)
  6. COMMIT
```

对外 API：

| API | 行为 |
|---|---|
| `forget_event(conn, id)` | `forget_events(&[id])` |
| `forget_session(conn, session_id)` | 收集该 session 全部 event id → `forget_events`；再删 trace_steps / turn_trace / tasks / sessions 行（顺序：先派生记忆与 events，再会话附属表，避免悬空） |
| `forget_all(conn)` | 保持「核按钮」语义：清空各层 + evidence 表；可直接 `DELETE FROM memory_evidence` 后沿用现有 batch（须把 `memory_evidence` 加入 batch） |

`memory/db/events.rs` 中旧 `forget_*` 改为调用 `forget::`，禁止第二份 DELETE 列表。

### 4.4 迁移与回填（v17 → v18）

1. `CREATE TABLE memory_evidence` + 索引（`migrate()` 自愈风格：IF NOT EXISTS）。
2. **回填 Fact**：`INSERT OR IGNORE … SELECT 'fact', id, source_event_id, 'extracted', ts FROM facts WHERE source_event_id IS NOT NULL`。
3. **回填 Boundary**：经 `source_fact_id → facts.source_event_id` 能连上的写入 `derived`；连不上的标孤儿（不删，见 §4.5）。
4. **回填 Profile**：非空 `source_event_id` → `extracted`。
5. **Preference**：无法可靠反查历史升格来源 → **不回填假 evidence**（I9）。新升格必须挂 evidence。

`PRAGMA user_version = 18`；changelog 写在 `schema.rs` 顶部与本文件。

### 4.5 孤儿 Preference 政策（产品诚实）

| 情况 | Forget Session 行为 |
|---|---|
| Preference 有 evidence，且本会话 events 是其**唯一**来源 | 删除 Preference + FTS |
| Preference 有多源 evidence | 只摘本会话 evidence，保留行 |
| Preference **零** evidence（历史孤儿） | **保留**；不在 Session Forget 中猜测删除 |
| `forget_all` | 删除全部（含孤儿） |

UI / 文案（若设置页有「忘掉会话」说明）：可加一句「早期未溯源的偏好可能仍保留，可用忘掉全部清除」——有入口再改文案，本方案不强制改 UI。

可选后续（非本阶段）：设置页「清理无来源记忆」扫孤儿——记入说明，不进本 PR。

### 4.6 Profile History

删除自动画像 key 时：

1. 将当前 `(key, value, source)` 插入 `user_profile_history`，`valid_until = now`；
2. `DELETE FROM user_profile WHERE key = ?`；
3. 不删除 history 里更早的行（审计保留；`forget_all` 才清 history）。

---

## 5. R2 — 失效 Fact 过滤（P1）

### 5.1 行为

| 路径 | 行为 |
|---|---|
| `beyond_window` | `if hit.invalidated { continue; }` |
| `memory_search` 默认装配进「事实」列表的行 | 跳过 invalidated |
| `db::search` | 增加参数或包装：`SearchOpts { include_invalidated: bool }`；**默认 false**（硬排除，不再仅 ×0.1） |
| 显式「历史回顾」 | 本阶段可不做独立 API；若评测需要，允许 `include_invalidated=true` 且格式化为 `历史事实（已失效）…`——默认产品路径关闭 |

### 5.2 软失效与 FTS

`invalid_at` 不删行 → FTS 仍可能命中。处理（选一，推荐 A）：

- **A（推荐）**：查询侧过滤 `invalidated`（R2 主路径）；索引暂留，避免夜间巩固大扫 FTS。
- **B**：invalidate 时同步删/改 `search_docs`——改动面大，本阶段不做。

M10 断言：构造 invalid fact + beyond/recall → 结果集不含其 content 作为当前事实。

---

## 6. R3 — 语义关系同义（P1）

### 6.1 模型

替换「大同义簇」为带类型边：

```rust
pub enum SemanticRelation {
    Synonym,
    EntityAlias,
    ParentCategory,
    ChildCategory,
    RelatedConcept,
    Opposite,
    CauseEffect,
}

pub struct RelationEdge {
    pub terms: &'static [&'static str], // 仍可用小簇，但整簇共享同一 relation
    pub relation: SemanticRelation,
}
```

### 6.2 扩腿规则

| 关系 | `expand_legs` |
|---|---|
| Synonym / EntityAlias | 整簇其余词进腿（与今日同） |
| Parent / Child / Related | **本阶段默认不扩**（宁漏勿反；Phase 2 再做加权候选） |
| Opposite | **禁止**扩 |
| CauseEffect | **禁止**扩 |

### 6.3 必拆现有簇（最低集）

| 原文簇 | 处置 |
|---|---|
| `蔗糖 / 加糖 / 不加糖` | 拆掉；`加糖`↔`不加糖` 标 Opposite（或不建边，仅确保不共簇） |
| `过敏 / 不能吃` | CauseEffect 或不建边 |
| `花生 / 坚果` | Parent/Child，本阶段不扩 |
| `舟山 / 海边` | RelatedConcept，本阶段不扩 |
| `小程序 / 应用` | Parent/Child，本阶段不扩 |

保留明显真近义（如 `跑步/慢跑`、`报销/报账`）。

### 6.4 反例评测（L2）

落在 `memory/synonym.rs` 或 `memory/eval_recall.rs`：

| 查询 | 不得因扩展而加入的腿 |
|---|---|
| 不加糖咖啡 | 加糖 |
| 坚果过敏 | 不得把「花生」当同义强行等价扩展出误导腿（本阶段不扩上下位即可） |

不要求本阶段改完所有生活簇——**先止血对立/因果，再收敛上下位**。

---

## 7. R4 — 主动配额持久化（P1）

### 7.1 表

```sql
CREATE TABLE IF NOT EXISTS proactive_daily_usage (
    local_date    TEXT PRIMARY KEY,  -- YYYY-MM-DD，本地日历日
    used_count    INTEGER NOT NULL DEFAULT 0,
    reserved      INTEGER NOT NULL DEFAULT 0,  -- 已预留未提交
    last_spoken_at TEXT
);
```

与 v18 同迁（R4 与 R1 可同 schema bump，即使分 PR 合入：先合 R1 占 v18，R4 用 v18 已有表或 v19——**推荐 R1+R4 同 bump v18**，减少连续迁移；若 R4 独立落后合入则 v19 只加此表）。

**推荐默认（D-R4-1）**：R1 与 R4 同版 **v18** 一次迁完，即使代码分 PR，迁移脚本一次带齐表。

### 7.2 状态机

```text
try_reserve(conn, local_date, limit) -> bool
  -- 原子：
  INSERT … (date, used=0, reserved=1)
  ON CONFLICT DO UPDATE
    SET reserved = reserved + 1
    WHERE used_count + reserved < limit
  -- changes()==0 → 配额满，Silent(QuotaExhausted)

commit_speak(conn, local_date)
  UPDATE … SET used_count = used_count + 1,
               reserved = MAX(reserved - 1, 0),
               last_spoken_at = now
  WHERE local_date = ?

release_reserve(conn, local_date)
  -- 生成/emit 失败时调用
```

`should_speak` 拆为：

1. Busy / idle / value 闸（纯函数，不碰配额）；
2. `try_reserve`；
3. 返回 `Speak` 仅表示「已预留」；
4. `proactive.rs` 在 **emit 成功** 后 `commit_speak`；失败 `release_reserve`。

内存 `ProactivityState`：可改为无配额字段，或仅作进程内单飞锁；**权威在 SQLite**。重启后 `reserved` 若残留：启动时 `UPDATE … SET reserved = 0`（崩溃预留作废，允许当日再次尝试——比永久吞配额更符合产品）。

### 7.3 日界

- 所有配额键：`clock::now_local().date_naive()`（测试 pin）。
- 删除 `should_speak` 内 `Utc::now().date_naive()`。
- `evaluate_live` 与 Speak 共用同一 `local_date` 变量。

### 7.4 单飞

`AppState` 保留 `Mutex` 包一层「主动评价中」或继续用现有 `proactivity` 锁包住 reserve→commit 全过程，避免双线程双预留；DB 原子是第二道防线。

---

## 8. R5 — 回归测试最低集

| ID | 场景 | 断言要点 | 挂靠 |
|---|---|---|---|
| M06 | 单会话表达偏好/边界/画像后 `forget_session` | 该会话唯一来源的 pref/boundary/profile 行为符合 §4.3；events/facts 已空 | R1 |
| M07 | 删除后 FTS / `db::search` | 对应 content 不再命中 | R1 |
| M08 | 两会话升格同一 preference content | 删会话 A 后 pref 仍在，且只剩 B 的 evidence | R1 |
| M10 | Fact `invalid_at` 已设 | `beyond_window` / 默认 search 不含为当前事实 | R2 |
| D01 | pin 本地日 vs UTC 已跨日 | 配额键为本地日 | R4 |
| D02 | 模拟重启（新 `ProactivityState` + 同 DB） | `used_count` 仍在，不可再 Speak | R4 |
| D03 | reserve 后失败 release | 仍可再次 Speak | R4 |
| S01 | 「不加糖」扩腿 | 不含「加糖」 | R3 |

测试位置建议：

- Forget / M06–M08 → `memory/forget.rs` 或 `memory/db/tests.rs`
- M10 → `memory_search` 单测
- D0x → `decision.rs` 单测（接 MemoryStore / 内存 SQLite）
- S01 → `synonym.rs`

门禁：`pnpm check`；相关 `cargo test` 过滤器在 PR 描述写明。

### 8.1 L3 最小真机清单（R1）

1. 新会话说清一条可升格偏好（或手动 consolidate）；
2. 确认设置/记忆侧能看到；
3. 删除该会话；
4. 新会话问同一偏好 → 不应再答出（孤儿历史数据除外，见 §4.5）；
5. 全文检索/回忆工具不应再命中已删会话原文。

---

## 9. 产品裁决表（推荐默认，实施按此执行）

| ID | 问题 | 推荐默认 |
|---|---|---|
| D1 | 孤儿 Preference 在 Session Forget 时？ | **保留**（I9） |
| D2 | manual Profile？ | **永不**因 Session/Event Forget 删除 |
| D3 | 无 evidence 是否允许新升格 Preference？ | **否** |
| D4 | search 默认是否包含 invalidated？ | **否**（硬排除） |
| D5 | Parent/Child 本阶段是否扩腿？ | **否** |
| D6 | 配额成功点？ | **前端事件 emit 成功**（或 DB 写入 proactive 对话事件成功——取「用户可见」更近的一侧；推荐 **emit 成功**，若 emit 成功但落库失败打错误日志且仍 commit，避免用户已看见却次日双发；落库失败必须可诊断） |
| D7 | schema | R1+R4 表一并进 **v18** |
| D8 | 分 PR | PR-A：R1+R2+R5(M06–08,M10)；PR-B：R4+D0x；PR-C：R3+S01 |

---

## 10. 里程碑与文件触点

### 10.1 里程碑

| 期 | 内容 | 完成定义 |
|---|---|---|
| **M0** | `memory_evidence` + 迁移回填 + `forget.rs` 骨架 + M06 红转绿 | ✅ L2：M06/M07/M08 |
| **M1** | 全部写入点挂 evidence；`events::forget_*` 改委托 | ✅ 无源升格 Err；onboarding 挂锚点事件 |
| **M2** | R2 过滤 + M10 | ✅ Leakage=0 |
| **M3** | R4 配额表 + reserve/commit | ✅ D01–D03 绿 |
| **M4** | R3 关系表 + 反例 | ✅ S01 绿；对立簇拆除 |
| **M5** | L3 真机清单勾选 | ⏳ 待真机（§8.1） |

### 10.2 预期文件触点（最小化）

| 路径 | 变更 |
|---|---|
| `memory/db/schema.rs` | v18 DDL + migrate |
| `memory/forget.rs` | **新建**权威 Forget |
| `memory/db/events.rs` | forget 委托；promote/add_* 挂 evidence |
| `memory/db/mod.rs` | mod 导出 |
| `memory/mod.rs` | 导出 forget |
| `memory/consolidate.rs` | promote 传 event ids |
| `memory/profile.rs` | upsert 写 evidence |
| `memory_search.rs` | beyond_window / search opts |
| `memory/synonym.rs` | Relation + 扩腿规则 |
| `decision.rs` / `dialogue/proactive.rs` / `lib.rs` | 配额持久化 |
| `memory/db/tests.rs` 等 | R5 |

前端：本阶段 **可不改**（命令名仍 `forget_session`）；若有「忘掉」说明文案再补一句孤儿政策。

---

## 11. 风险

| 风险 | 等级 | 应对 |
|---|---|---|
| 迁移后大量孤儿 pref，用户以为「删会话等于全忘」 | 中 | 文案诚实；forget_all 仍在；可选清理工具后期做 |
| `ON DELETE CASCADE` 与手工删序打架 | 中 | 单测覆盖顺序；先裁决派生再删 event |
| 配额 reserve 崩溃残留 | 低 | 启动清 `reserved=0` |
| 同义收紧导致召回率掉 | 中 | 只禁 Opposite/CauseEffect/上下位；真近义保留；eval 基线对比 |
| 同 PR 过大 | 中 | 按 D8 拆三个 PR，schema 一次到位 |

---

## 12. 与 Roadmap / 其他文档关系

- 上级：[`roadmap-architecture-r2/v1`](../../roadmap-architecture-r2/v1/index.md) Phase 1。
- 能力扩展线：[`roadmap-agent-capabilities.md`](../../roadmap-agent-capabilities.md) 不替代本方案；冲突时本方案阻塞记忆相关新能力。
- 旧记忆方案：[`plan-memory.md`](../../plan-memory.md) 的「溯源不变式」在本方案用 `memory_evidence` **落地为可执行结构**；不覆盖历史文档，以本版为 Forget/Provenance 权威。

---

## 13. 变更记录

| 版本 | 日期 | 说明 |
|---|---|---|
| v1 | 2026-11-24 | 初稿：R1–R5 详细设计、裁决默认、里程碑与测试最低集 |
| v1.1 | 2026-11-24 | 按推荐默认实施：schema v18、`memory/forget`+`evidence`、R2–R4、同义分型、M06–M10/D0x/S01 |

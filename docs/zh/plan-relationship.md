# 方案：关系线成长（MVP P2 第 1 项）

> **状态**：Q1–Q4 已实施（2026-11-19，140 单测全过）；R2 的「缓冲句命中率下降」真机验证已通过（2026-11-20）
> **日期**：2026-11-19
> **依赖**：预测-验证循环（predict.rs + predictions 表，已落地）、形态线状态机与 60s 轮询模式（morph.rs，已落地）、反谄媚过滤器（generation.rs，已落地，本方案补落库）、人格渲染器与 `disagreement_policy`（persona，已落地）
> **对应调研**：docs/zh/research-report.md §5.3 反谄媚四条、§6.2 三条成长曲线的节奏、§6.3 决策 2/3、§8.3 P2 第 1 项

---

## 1. 目标与非目标

### 目标

1. **第 8–30 天有一件真正发生变化的事**：从「不敢反驳」到「会顶嘴」（调研 §6.2 原话——这个转变的戏剧性最强，也是关系线的全部内容）。
2. **靠理解准确度推进，不靠时间也不靠签到**（§6.3 决策 2）：关系前进的证据只有一个——它猜对了你在做什么，而且你敢让它猜。
3. **每次升级改变交互方式而不只是加标签**（§6.3 决策 3）：R1 换语气与引用方式，R2 解锁「直接说不同意」的表达许可——升级能被讲成一个故事，而不是一个数字。
4. **不惩罚用户行使控制权**：「别烦我」是合法反馈，不扣分、不掉阶，只让推进「慢下来」（宁可不猜，也别烦）。

### 非目标（P2 明确不做）

- **不做亲密度数值 / 好感度条 / 等级徽章 / 排行榜**——这是调研点名反对的反模式（§6.1：数值机制边际效用衰减极快，第 20 个徽章就是家具）。
- **不做「关系等级解锁功能」**：P0/P1 已有能力全部默认可用，关系线只改「说话方式」，不减任何东西（与形态线同一红线）。
- **不做第 8 天前的任何关系线痕迹**：day 0–7 是形态线的舞台，关系线在 UI 上完全不存在（只在后台记证据）。
- **不改形态线节奏**（S3 在 day 6），也不合并成一条总等级。
- **不做云端同步 / 跨设备关系状态**（P2-5 另议；关系数据是本机资产）。
- **不为触发「第一次拒绝你」而伪造反对场景**：没有真实分歧就不硬造，宁可这个彩蛋永不出现。

---

## 2. 现状盘点（写方案前的代码事实）

| 已有 | 位置 | 对本方案的价值 |
| --- | --- | --- |
| `predictions` 表（date/guess/basis/feedback/resolved_at） | `memory/db.rs` §schema v3 | **理解准确度的唯一硬来源**；可按 date 聚合 hit/miss/shut_up |
| `db::prediction_stats()`（accuracy / accuracy_7d / backoff_rate） | `memory/db.rs` | 退避率与命中率的现成口径，直接复用不另造指标 |
| `growth.born_at` 出生锚点（从最早事件回填） | `memory/db.rs` §v4 | 关系线与形态线**共用一个时间原点**，避免两条时间线错位 |
| 60s 轮询 + `last_tick_date` 每日幂等 | `morph.rs` | 推进一步一次的模式现成，照抄骨架 |
| `growth_log` + `notified_at` 开窗播报队列 | `morph.rs` / `db.rs` | 升阶播报的幂等通路现成，复用同一套「不弹窗、只在开窗时说」 |
| `generation::filter()`（无条件肯定 / 缓冲式反对两张词表） | `generation.rs` | R2「会顶嘴」的度量基础；**但它只返回 bool，不落库**（见缺口） |
| `disagreement_policy`（per_turns 5 / min_substantive 1 / banned_forms 4 条） | `resources/persona.json` | 反对预算已机器可读，R2 把它从「建议」升为「硬约束」 |
| `system_prompt()` 注入 boundaries + mood | `dialogue.rs` | 加一段 `relationship` 模式的挂载点现成 |
| ACTIVITY_KINDS（dialogue / perception / prediction_*） | `morph.rs` | 「在场」判定与生疏判定共用同一口径，不另造一套 |

| 缺口 | 说明 |
| --- | --- |
| 关系状态 | 无「关系阶段 + 升级时间 + 生疏」的持久化与推进逻辑 |
| 理解证据 | 命中/猜错/顶嘴散落在三张表（predictions / events / 内存），没有按日聚合的可回放记录 |
| 反谄媚落库 | filter 结果只回给前端展示，**不写 events**——「它顶了几次嘴」无法统计，R2 的验收指标无从谈起 |
| 提示层模式 | prompt 里没有「关系模式」，R2 无法让模型真的改用直说语气 |
| 关系事件 | 没有「第一次拒绝你」这类可讲成故事的审计记录 |

---

## 3. 核心机制设计

### 3.1 三阶段 × 8–30 天节奏

```text
R0 拘谨     day 0–8      UI 上不存在；后台只记证据（与形态线并行，不抢戏）
R1 熟络     day ≥ 9 且证据分 ≥ 12    语气放开、引用变「你上次那个事」
R2 会顶嘴   day ≥ 22 且证据分 ≥ 30   解锁「直接说不」，反对不再加缓冲句
```

节奏依据（§6.2）：三线错开，避免同一天「全部解锁完」——

```text
day 6  S3 成景（形态线收官）
day 9  R1 熟络（最快，正常参与的用户）
day 12 R1（只来不答的用户，证据不够就延后）
day 22 R2 会顶嘴（最快）
day 27 R2（慢热用户）
```

**门槛是「双条件」而非「天数」**：天数到了证据不够，就等；证据够了天数没到，也等。这正是「靠理解准确度」的字面意思——时间只提供窗口，不提供进度。

### 3.2 「理解证据」如何量化（内部叫理解证据，不叫亲密度）

每日一次聚合，三条正向证据 + 一条温和负项：

| 证据 | 判定 | 当日上限 |
| --- | --- | --- |
| **在场** | 当日有 `dialogue` 或 `prediction_feedback` 事件 | +1 |
| **猜中** | 当日 `hit` 次数 ≥ 1 | +1 |
| **顶嘴没被拒** | 当日有 `disagreement` 事件且当日 `shut_up` = 0 | +1 |
| **猜错** | 当日 `miss` 次数 ≥ 1 | −1（下限 −1，不累积成负） |

关键设计取舍：

- **`shut_up` 不扣分**。用户点「别烦我」是在行使控制权，惩罚它等于训练用户不敢点。它只影响下一节的「推进暂停」。
- **猜错只扣 1 且不可累积**：敢猜本身有价值，猜错是成本不是罪过。这个 -1 的作用只是让「乱猜」拿不到分。
- **「顶嘴没被拒」是唯一的质性证据**：它同时要求「模型确实反对了」和「用户没有嫌烦」——这正是关系线要的东西，量少但权重和猜中一样。
- 单日 score ∈ [−1, 3]，实际分布在 0–2；正常参与约 1.5/天，只来不答约 1/天。

**分数永远不暴露**：不进 API 返回值、不上 UI、不写日志摘要。它只是阶段判定函数的中间量，对外只有 `stage` + `stage_name`。（这是「不做数值游戏」的工程保证，不是口头承诺。）

### 3.3 Schema v6：三张表（状态 / 每日得分 / 审计）

```sql
-- 单行关系状态。注意：不存 born_at —— 复用 growth.born_at（同一天出生）
CREATE TABLE IF NOT EXISTS relationship (
    id             INTEGER PRIMARY KEY CHECK (id = 1),
    stage          INTEGER NOT NULL DEFAULT 0,   -- 0..2
    stage_since    TEXT,                          -- 当前阶段开始时间（「第一次拒绝你」的 7 天窗口）
    cold_since     TEXT,                          -- 生疏开始时间；NULL = 温度正常
    last_tick_date TEXT                           -- 本地日；阶段推进每天只算一次
);

-- 每日证据聚合：可回放、可解释、可测试
CREATE TABLE IF NOT EXISTS relationship_progress (
    date          TEXT PRIMARY KEY,               -- YYYY-MM-DD（天然幂等）
    present       INTEGER NOT NULL DEFAULT 0,
    hits          INTEGER NOT NULL DEFAULT 0,
    misses        INTEGER NOT NULL DEFAULT 0,
    disagreements INTEGER NOT NULL DEFAULT 0,
    shut_ups      INTEGER NOT NULL DEFAULT 0,
    score         INTEGER NOT NULL DEFAULT 0,
    computed_at   TEXT NOT NULL
);

-- 审计：一行一次升阶，notified_at 保证只播一次
CREATE TABLE IF NOT EXISTS relationship_log (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    stage       INTEGER NOT NULL,
    unlocked_at TEXT NOT NULL,
    notified_at TEXT,
    note        TEXT
);
```

- **每日得分按「重算」而非「累加」**：每次 tick 都从 `predictions` + `events` 重新推导当天得分并 UPSERT。源数据不变则结果不变（幂等），改错了也能自己纠正——比增量计数器可靠得多。
- **阶段判定不含今天**：`cum = SUM(score) WHERE date < today`。今天还没过完，不参与升阶——避免「今天猜中一次就立刻升级」的浮躁感。
- **`forget_all` 级联清空三张表**；`forget_session` 不动它们（关系是跨会话事实，不属于单会话记忆）。

### 3.4 推进、暂停、生疏

```text
每日 tick（last_tick_date 幂等）
  ① 重算今天得分 → UPSERT relationship_progress
  ② 若 last_tick_date != today：
       cum = SUM(score) WHERE date < today
       if 近 7 天 backoff_rate > 10%  → 暂停推进（记 note，不写 stage）
       elif day ≥ 9  and cum ≥ 12 and stage < 1 → 升 R1
       elif day ≥ 22 and cum ≥ 30 and stage < 2 → 升 R2
       last_tick_date = today
  ③ 生疏判定（每 tick）：7 天无 activity → cold_on；任意 activity → cold_off
```

三条不变式（代码强制，测试锁死）：

1. **阶段只进不退**——生疏降的是「温度」，不是阶段；暂停只是停，不是退。
2. **暂停有解释**：暂停时写一条 `note`（「最近你老让我别烦，我先不往前凑了」），可在工具页看到，不静默。
3. **生疏可逆**：第一次互动当 tick 恢复，和形态线 dormant 完全同构（同 ACTIVITY_KINDS、同为外观/语气状态、同样不锁功能）。

### 3.5 每阶段一处交互变化（决策 3 的落地）

| 阶段 | 交互变化 | 实现方式 |
| --- | --- | --- |
| **R0 拘谨** | 现状：礼貌、有距离、反对要加缓冲句 | prompt 注入 `relationship: reserved` |
| **R1 熟络** | 语气放开：允许口语、允许「你上次说的那个」式引用；poke 文案换一套；主动搭话从「试探」变「随口」 | prompt 注入 `relationship: familiar`（L2 表达层放行口语化）；poke 模板池按阶段切换 |
| **R2 会顶嘴** | **反对不再加缓冲句**：`banned_forms` 从「建议避免」升为硬约束；prompt 明确「不同意就直说，说清你不同意哪一点」 | prompt 注入 `relationship: direct` + `disagreement_policy` 强化段；filter 的 hedged 命中在 R2 下记为负样本事件 |
| **生疏（cold）** | 语气回落到客气，poke 变「……哦，是你。」；回来几次即回暖 | prompt 注入 `relationship: rusty` |

- 关系模式与 `mood` 正交：一个管「此刻情绪」，一个管「长期关系」；prompt 里两段独立拼接，互不覆盖。
- **「第一次拒绝你」彩蛋**：R2 达成后 7 天窗口内（`stage_since` 起算），若出现一次 hedged 命中且当日无 `shut_up`，播报一条「我今天本来想顺着你说，想了想——不。」。这是「关系有张力」的可讲时刻，也是反谄媚从规则变成性格的证据。若窗口内没发生，不补、不造。

### 3.6 补缺口：反谄媚证据落库

现状 `filter()` 只返回两个 bool，前端展示完就丢了。本方案加第三个检测与两条事件：

```rust
// generation.rs：新增一条词表（只做证据记录，不做拦截）
pub const EXPLICIT_DISAGREEMENTS: &[&str] = &[
    "我不同意", "我不这么看", "我反对", "这么说有问题",
    "这个判断不对", "我不同意这个说法", "I disagree", "I don't think so",
];

pub struct FilterReport {
    pub unconditional_affirmation: bool,
    pub hedged_disagreement: bool,
    pub explicit_disagreement: bool,   // 新增
}
```

`dialogue::chat_turn_inner` 在 filter 之后写事件（仅当命中，避免噪音）：

- `explicit_disagreement` 命中 → `add_event("disagreement", <回复前 80 字>)`
- `hedged_disagreement` 命中 → `add_event("disagreement_hedged", <回复前 80 字>)`

这两条事件直接喂 §3.2 的「顶嘴没被拒」证据，也是 persona 一致性回归的未来样本库。

> 注意：`disagreement` / `disagreement_hedged` / `relationship_stage` / `relationship_cold` **都不在 ACTIVITY_KINDS 里**——它们是派生写入，若算作 activity 会导致「自己陪自己」，生疏判定永远不触发（形态线的 `face_state` 就是这个坑）。

### 3.7 老用户回填

- **回填每日得分**：首次建表时，按 `predictions` 表历史 + `events` 表历史重算最近 30 天的 `relationship_progress`。逻辑与 tick 完全同一函数，保证「回填 = 重放」。
- **不回填 stage**：stage 仍从 0 起，首次 tick 按真实 cum 判定——可能一次跳到 R2。此时播报用「回头看看」的语气（「其实我们处了一阵子了。行吧，那就不客气了。」），且**只播最高阶一条**。
- 理由与形态线出生锚点回填一致：老用户不该「重新认识」一遍，但也不该被偷偷跳过所有过程——所以在开窗那一刻把话说明白。

### 3.8 前端（极克制）

关系线**不做独立面板、不做进度条、不做阶段徽章**：

- **升阶播报**：复用形态线的聊天流气泡 + `notified_at` 幂等，挂在 `window:shown` 断点，不占每日打扰额度。
- **工具页一行**：「它对你的理解：熟络 · 第 14 天」，点开只有一句解释和「重置关系线」按钮（走 `forget_all` 的关系线分支，不做全清）。
- **生疏态**：只在 caption / poke 文案上体现，不加任何 UI 元素。
- **预览/演示位**：表情页不加形态演示之外的按钮；关系阶段可用工具页调试按钮切换（仅 dev 构建可见）。

---

## 4. 数据流总览

```text
每分钟 tick（复用 morph 的轮询线程，独立支路）
   │
   ├─ ① 聚合今天：predictions(date=today) + events(kind ∈ dialogue/disagreement/shut_up 口径)
   │      → UPSERT relationship_progress（幂等重算）
   │
   ├─ ② 每天一次：cum = SUM(score) WHERE date < today
   │      ├ backoff_rate > 10% → 暂停（写 note，不升阶）
   │      └ day/cum 双达标 → stage++ → relationship_log（待播报）+ add_event
   │
   ├─ ③ 每 tick：7 天无 activity → cold_on / 有 activity → cold_off
   │
   └─ 变化 → emit relationship:stage（窗口开着即时更新）
                          ↓
        开窗时 relationship_status → pending 播报气泡 → relationship_ack（幂等）
                          ↓
        dialogue::system_prompt 读取 stage → 注入 relationship 模式段
```

指标（常驻 CI，纯函数 + 临时库即可测）：

- **升阶准时率**：day 9 + cum 12 必升 R1；day 9 + cum 11 必不升；day 8 + cum 99 必不升（天数未到）。R2 同理。
- **退避暂停正确率**：近 7 天 backoff 15% 时 7 日内不升阶，且 stage 不退；必失败反例是「暂停后又偷偷升了」。
- **生疏正确率**：7 天无 activity 必 cold、当 tick 恢复、stage 不变；`disagreement` 等派生事件不得算作 activity。
- **证据记录正确率**：filter 三个 bool 与事件写入一一对应，同一回复重复调用不去重写两条（按 session + 内容哈希去重可选，P2 先不做）。

> 「R2 下 hedge 命中率必须低于 R0」是**产品假设，不是 CI 指标**——CI 里跑的是 MockBackend，验证不了真模型行为。这一条进真机评测清单，明确标注为「待真模型验证」。

---

## 5. 里程碑

| 里程碑 | 内容 | 规模 | 依赖 | 状态 |
| --- | --- | --- | --- | --- |
| **Q1** | schema v6（relationship / relationship_progress / relationship_log）+ `forget_all` 级联 + 30 天回填（重放函数） | 0.5 周 | 无 | ✅ |
| **Q2** | 每日证据聚合（纯函数 `score_day`）+ 阶段推进（双条件 + 暂停 + 单调）+ 生疏判定/恢复 | 0.5 周 | Q1 | ✅ |
| **Q3** | prompt 关系模式注入（reserved/familiar/direct/rusty）+ `FilterReport.explicit_disagreement` + 两条证据事件落库 | 1 周 | Q2 | ✅ |
| **Q4** | `relationship_status` / `relationship_ack` / `relationship_reset` 命令 + 开窗播报 + 工具页一行 + 「第一次拒绝你」彩蛋 + 四指标 CI + 文档 | 0.5 周 | Q3 | ✅ |

**实施备注**（与方案的偏差）：

- 累积证据加了 **30 天滚动窗口**（方案 §3.3 原文只写 `date < today`）：与回填窗口一致，
  语义统一成「近一个月的理解证据」，避免早期证据永久累积把后期用户自动推上 R2；
- 回填做成**每次启动都跑的滚动重放**（不写「已回填」标记）：幂等且自愈，代价是启动时
  多几十次只读查询；
- `relationship_log` 增加了 `kind` 列（`stage` | `first_refusal`），让彩蛋播报与升阶播报
  共用同一条「只播一次」的通道，而不是另建一张表；
- 「第一次拒绝你」的触发条件从「R2 后 7 天内有 hedged 命中」收紧为**同时**要求当日
  `shut_up = 0`——用户当天嫌烦，就不算「没被拒」；
- R2 的「缓冲句命中率必须低于 R0」没有进 CI（MockBackend 验证不了真模型行为），
  按方案标注为真机评测项。

**Q1–Q4 全部不依赖真模型**（证据聚合与推进是纯数据推导），prompt 侧效果需真机验证。

**与形态线的复用清单**（避免重写）：轮询线程挂载点、`last_tick_date` 幂等模式、`notified_at` 播报队列、`ACTIVITY_KINDS` 口径、`clock::pinned` 测试时钟、临时库测试 helper。

---

## 6. 开放问题（待评审）

1. **门槛数值 12 / 30 是否合适**？按「正常参与 1.5 分/天」推算：day 9 ≈ 13.5（准点 R1）、day 22 ≈ 33（准点 R2）；只来不答约 1 分/天 → R1 延到 day 12、R2 延到 day 30。建议先固定，用真实数据观察一个版本再调，不做设置项。
2. **老用户回填 30 天是否会让部分人一升级就是 R2**？建议接受（诚实优先），并用「回头看」语气播报；备选是把回填上限压到 14 天，但会让「理解了很久」这件事失真。
3. **关系线是否要在 UI 可见**？建议只做工具页一行 + 升阶播报，**不做独立面板**——一旦有了面板，用户就会盯进度，关系就变回了数值游戏。
4. **`explicit_disagreement` 用词表够不够**？中文反对表达极多样（「未必吧」「这说不过去」）。建议词表只做**证据记录**不做拦截，宁可少记（宁缺毋滥），后续用真机数据补词；prompt 侧才是主要抓手。
5. **「第一次拒绝你」要不要更主动**？可选方案是 R2 达成当天主动问一句「你有没有觉得我最近太顺着你了？」——不建议，这是诱导用户承认分歧，属于表演。

---

## 7. 合规与隐私对照

- 关系判定只用本机已有数据（`predictions` 与 `events` 的时间戳/判定结果），**不新增任何采集**；不读屏幕、不读内容语义。
- 「理解证据分」只在本机推导、不落展示、不上传；对外只暴露阶段名与天数。
- 不签到、不连续天数奖励、不惩罚性数值——「别烦我」不扣分是硬性实现（有测试锁），符合「不得诱导沉迷」方向。
- 生疏是情绪表达而非惩罚：不锁功能、不降阶段、一句话就回暖。
- `forget_all` 级联清空三张关系表；`relationship_reset` 只清关系线、不动记忆与形态（用户的「重新认识」应当是分项可控的）。
- 关系数据不进导出包（P2-5 若做加密导出，关系状态默认排除，需单独 opt-in）。

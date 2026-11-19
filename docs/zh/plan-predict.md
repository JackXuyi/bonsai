# 方案：预测-验证循环（MVP P1 第 1 项）

> **状态**：G1–G4 已实施（2026-11-19，98 单测全过）；G5（LocalLmPredictor）挂起等真模型验证
> **日期**：2026-11-19
> **依赖**：三层记忆（plan-memory.md，已落地）、主动搭话通道（dialogue.rs，已落地）、人格渲染器（已落地）
> **对应调研**：docs/zh/research-report.md §6.3 决策 2、§8.3 P1、§9 实验 5

---

## 1. 目标与非目标

### 目标

1. 落地「每天猜一次」循环：阿盆基于记忆层做出一个**可验证的预测**
   （「你接下来大概要去……」），用户给三档反馈：**对 / 错 / 别烦我**。
2. 反馈回写记忆：命中 → 对应偏好 confidence 上升（streak 记录）；错 → 下降；
   「别烦我」→ 预测退避 N 天（沉默是显式动作，且用户忽略也应记为负样本）。
3. 建立两个一等指标：**预测命中率**（7 日滚动）与**退避率**，进常驻 CI 评估。
4. 为 P2 关系线成长预留数据地基：连续命中 streak 是未来「解锁反驳权限」的
   叙事素材，本方案只记数据、不做成长动画（那是 P1 第 2 项形态线与 P2 的事）。

### 非目标（P1 明确不做）

- 不做关系线/能力线成长解锁（P2）。
- 不做成长动画与形态变化（P1 第 2 项「形态线成长」单独做）。
- 不做多预测并列（每天最多 **1** 条，宁缺毋滥——预测过多会退化为通知轰炸）。
- 不做云端生成（预测生成走规则 + 本地 LLM 双轨，与记忆 M5 同构）。

---

## 2. 现状盘点（写方案前的代码事实）

| 已有 | 位置 | 对本方案的价值 |
| --- | --- | --- |
| preferences 表（category ∈ like/habit/context/boundary，带 confidence/invalid_at） | `memory/db.rs` | 预测的**证据源**：habit 类偏好就是「什么时候干什么」的素材 |
| events 表（kind 含 perception，带 ts） | `memory/db.rs` | 行为模式证据：App 使用时段分布、剪贴板/下载节律 |
| 主动搭话线程 + `proactive:message` 事件 + `dialogue_proactive_now` | `dialogue.rs` | 预测的**投放通道**现成，前端监听现成 |
| L4 决策模型（每日配额、idle、busy 不打断） | `decision.rs` | 时机判定复用；预测配额独立计数 |
| 人格渲染器（L1 前置 + 记忆边界注入） | `persona/prompt.rs` | 预测措辞要走人格卡（阿盆的猜法应该「有点脾气」） |
| 本地 LLM 客户端 + 降级模式 | `llm.rs` + M5 | LocalLmPredictor 与 RulePredictor 同 trait 双轨 |
| 时钟可注入 | `memory/clock.rs` | 预测调度与指标测试可确定性 |
| consolidate 审计表模式 | `consolidate_runs` | predictions 也按审计表模式建表 |

| 缺口 | 说明 |
| --- | --- |
| 预测生成 | 无任何「从 habit/行为证据产出猜测」的通路 |
| 反馈收集 | 前端无三档反馈控件；后端无反馈写入与回传记忆的通路 |
| 退避机制 | 「别烦我」后暂停预测的调度逻辑不存在 |
| streak/指标 | 无命中率、退避率的统计与评估 |

---

## 3. 核心机制设计

### 3.1 一条预测的生命周期

```text
生成（每日 1 次窗口内，证据不足则今天不猜）
   │
   ▼
投放（复用 proactive 通道；措辞走人格渲染器）
   │
   ▼
等待反馈（48h 未回应 = 未验证，不影响命中率，只记 coverage）
   │
   ├── 对    (hit)      → habit confidence +0.1（封顶 0.95）+ streak += 1
   ├── 错    (miss)     → confidence −0.1（下限 0.3）+ streak 清零
   ├── 别烦我 (shut_up) → confidence −0.15 + 预测退避 3 天 + 记强负样本
   └── 忽略  (no_reply) → 仅记 coverage，confidence 不动
```

设计不变式（与记忆方案同风格，代码强制）：

1. **预测永不静默改写**：反馈只追加（feedback 列 + feedback_ts），历史行不可变。
2. **宁缺毋滥**：无证据（没有 habit 偏好且当日行为信号 < 阈值）→ 今天不猜，
   记 `skipped`，绝不硬编一条猜。装懂比沉默更伤人。
3. **预测 ≠ 聊天配额**：预测有独立 1/天配额，不占用每日主动搭话的额度；
   但两者共用「busy 不打断 + idle 条件」的 L4 判定，一天最多打扰一次总量
   由 L4 统一裁决（预测与搭话互斥：已主动说过话的日子不再预测）。

### 3.2 预测生成器：`PredictionGenerator` trait

与 `FactExtractor` 同构的解耦模式：

```rust
pub struct PredictionCandidate {
    pub guess: String,          // 人话措辞（经人格渲染）
    pub basis: Vec<i64>,        // 证据溯源：preference ids / event ids
    pub ask_after_hour: u32,    // 建议投放时段（本地时区）
    pub confidence: f64,
}

pub trait PredictionGenerator {
    fn predict(&self, digest: &PredictionDigest) -> Option<PredictionCandidate>;
}
```

- **RulePredictor（P1 立即可用，无模型也能跑）**——三条确定性规则：
  1. **habit 直推**：取 confidence 最高的 habit 偏好（如「用户 10 点左右开
     会议软件」），在匹配时段生成猜测；
  2. **节律外推**：统计近 14 天 events 中同一 App/剪贴板主题的出现时段，
     出现 ≥5 天且今日临近该时段 → 生成「你今天大概也要……」；
  3. **连续性猜**：昨天有 perception 记录的 App 今天同时段还没开 →
     「平时这个时候你已经在用 X 了」（带阿盆式措辞，可被嫌弃，这是特性）。
  规则按 habit 直推 > 节律外推 > 连续性猜优先级取第一条；多条冲突时取
  confidence 最高。`basis` 必须非空（可解释性不变式：每次猜测能说出依据）。
- **LocalLmPredictor（挂起，等真模型验证后接入）**：把 habit + 节律摘要喂给
  本地模型产出更自然的措辞，失败降级 RulePredictor——与 M5 降级模式一致。

### 3.3 调度：复用 60s 轮询，新增预测窗口

- 触发窗口：候选的 `ask_after_hour` ± 1h（本地时区），且该日尚未投过预测；
- L4 前置判定复用 `evaluate_live` 的 idle/busy 逻辑；busy → 推迟到窗口尾，
  仍 busy → 今天跳过（不打断是铁律）；
- 与主动搭话互斥由 `ProactivityState` 扩展一个 `spoke_today` 检查实现；
- 「别烦我」退避状态持久化在 predictions 元数据（`backoff_until`），
  重启不丢失。

### 3.4 Schema v3：`predictions` 审计表

```sql
CREATE TABLE IF NOT EXISTS predictions (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    date          TEXT NOT NULL,          -- 本地日 YYYY-MM-DD，一天一行上限
    guess         TEXT NOT NULL,
    basis         TEXT NOT NULL,          -- JSON: [{type:"preference"|"event", id}]
    ask_after_hour INTEGER NOT NULL,
    asked_at      TEXT,                   -- 实际投放时间；NULL = skipped
    skip_reason   TEXT,                   -- 'no_candidate' | 'busy' | 'spoke_today' | 'backoff'
    feedback      TEXT,                   -- 'hit' | 'miss' | 'shut_up'（NULL = 未回应）
    feedback_at   TEXT,
    resolved_at   TEXT,                   -- 48h 窗口关闭时间
    UNIQUE(date)
);
```

幂等：`UNIQUE(date)` + 投放前查当日行；`PRAGMA user_version` 升 v3，
沿用现有迁移器。合规：`forget_all` 级联清空 predictions。

---

## 4. 反馈 → 记忆回写（这是循环的另一半）

| 反馈 | 记忆层动作 | 说明 |
| --- | --- | --- |
| 对 | `bump_preference(confidence +0.1)`；streak 表累计 | 偏好被现实验证，升格门槛（≥0.8）天然受益 |
| 错 | `bump_preference(confidence −0.1)` | 低于 0.5 的 habit 会被夜间整理的删除清扫自然淘汰 |
| 别烦我 | confidence −0.15 + `backoff_until = now+3d` | 用户忽略（48h 无反馈）也记为弱负样本计数，进指标 |
| — | 每条反馈同时写一条 events（kind=`prediction_feedback`） | 供成长叙事与调试；事件层仍是唯一 ground truth |

**streak 与指标**（`predictions` 表可导出，常驻 CI 测试）：

- **命中率** = hit / (hit+miss)，7 日滚动，目标 **≥50%**（规则版起步，
  达不到说明证据源不对，先调规则而不是硬上模型）；
- **退避率** = shut_up / asked，目标 **<10%**（超过即产品性打扰，要回退）；
- **覆盖率** = asked / eligible days，仅观测不设门槛（诚实：证据不足的日子
  跳过是对的）。

---

## 5. 里程碑

| 里程碑 | 内容 | 规模 | 依赖 |
| --- | --- | --- | --- |
| **G1** | schema v3 迁移 + predictions 表 + forget 级联 | 0.5 周 | 无 |
| **G2** | PredictionGenerator trait + RulePredictor 三规则 + PredictionDigest（habit 查询 + 14 天节律统计） | 1 周 | G1 |
| **G3** | 调度接入（预测窗口 + L4 复用 + 互斥 + 退避）+ 投放（人格渲染措辞 + proactive:message 复用） | 0.5 周 | G2 |
| **G4** | 反馈命令 `prediction_feedback` + 记忆回写 + 三指标 + eval 常驻测试 + 前端三档按钮 | 1 周 | G3 |
| **G5**（挂起） | LocalLmPredictor：本地模型产出措辞与猜测，降级 RulePredictor | 0.5 周 | 真模型验证 |

**G1–G4 不依赖真模型，可立即开工**；与 P1 第 2 项（形态线）共享「本地日 +
天数统计」的地基，建议 G1 的 schema 一并预留形态线所需字段。

## 6. 开放问题（待评审）

1. **「别烦我」退避 3 天是否够**？建议 P1 固定 3 天不做设置项，观察退避率再调。
2. **预测与主动搭话互斥**（说话的日子不猜）是否太保守？建议先互斥——
   一天最多打扰一次是 6.4 节的红线，宁可少说。
3. **RulePredictor 措辞是否够「阿盆」**：规则版用固定模板 + 语气变体
   （3 套），真模型接入后换 LocalLmPredictor。P1 不为此阻塞。

---

## 7. 合规与隐私对照

- 预测只使用本机数据（preferences/events），措辞可含 App 名但不外发；
- `forget_all` / `forget_session` 覆盖 predictions（合规第 16 条延伸）；
- 「别烦我」是一等反馈而非隐藏开关，符合「把沉默当显式动作」的设计哲学。

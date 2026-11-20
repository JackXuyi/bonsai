# spec-m3 — 记忆 tab（只读总览）

> 依赖：M0b（可与 M1/M2 并行）。目标：记忆库可看不可改（边界清单除外）。原则沿袭 plan-relationship：**只展示，不提供编辑**——记忆可看是信任，可改是作弊。

## 1. Rust：`memory_overview`（新命令，放 `memory/mod.rs`）

```rust
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryOverview {
    counts: MemoryCounts,          // 见下
    recent_events: Vec<EventRow>,  // 最近 limit 条，时间倒序
    recent_facts: Vec<FactRow>,
    recent_preferences: Vec<PrefRow>,
    recent_predictions: Vec<PredictionRow>,  // 最近 1 条 + 反馈状态
    last_consolidate: Option<ConsolidateRunRow>,
}
pub struct MemoryCounts {
    events: i64, events_by_kind: Vec<(String, i64)>,  // SQL GROUP BY kind
    facts: i64, preferences: i64, boundaries: i64,
    predictions_total: i64, predictions_hit: i64,     // 命中率分子分母
    tool_calls: i64, calendar_cache: i64,
}
```

- 签名：`memory_overview(limit: Option<u32>)`，limit 默认 20、上限 100（clamp）。
- 实现：单连接只读查询（`MemoryStore` State 借 conn），纯 SELECT，无写；分页 v1 不做（limit 顶格够用）。
- `events_by_kind` 含派生事件（face_state / disagreement* / relationship_*）——前端灰显标注（复用 plan-relationship 的「派生不进 ACTIVITY_KINDS」认知，展示层如实呈现）。
- 时间字段：RFC3339 原样返回，**前端本地化渲染**（后端不做时区转换，参照 predict.rs 的教训：时间比较只在 Rust 侧，展示转换在前端）。

## 2. 前端（`/memory` 页，M0b 占位替换）

```
components/memory/
├── MemoryOverview.tsx      # 组装：徽章行 + 分区
├── CountBadges.tsx         # 计数徽章（events/facts/prefs/boundaries/prediction 命中/tool_calls/calendar）
├── EventList.tsx           # 最近事件（kind 等宽字体 + 派生灰显 + 相对时间）
├── FactsPrefsList.tsx      # 事实/偏好/边界分区（可折叠，shadcn Collapsible）
├── PredictionCard.tsx      # 最近一次预测 + 反馈状态（复用对话页 PredictionCard 的只读形态）
└── MemoryActions.tsx       # 整理记忆 / 忘记全部
hooks/useMemoryOverview.ts # 拉取 + 进页刷新
```

- 交互：进页拉取（`useMemoryOverview`）；「整理记忆」按钮调现有 `consolidate`（跑完 toast + 重拉）；「忘记全部」**两段式确认**（沿用设计稿交互：首次点击变「确认忘掉一切？」，4s 未确认回弹）→ `forget_all` → 全页重拉 + toast「已忘记一切」。
- 预测命中率徽章：`predictions_hit/predictions_total`，分母 0 显示「还没有预测」。
- 边界区只展示计数与条目（复用 `boundary_list`），删除入口**不重复**——编辑仍只在工具页边界清单。

## 3. 测试清单

Rust（cargo test，~6）：
1. 空库：各计数 0、列表空、`last_consolidate` None（1）
2. kind 分组正确（含派生事件计入 events）（1）
3. limit clamp：超 100 按 100（1）
4. 只读性：调用前后 DB 内容逐表不变（内存库快照对比）（1）
5. predictions 命中数与 feedback 状态一致（1）
6. forget_all 后 overview 归零（1）

前端（vitest + RTL，~5）：
1. `CountBadges`：0 分母「还没有预测」（1）
2. `EventList`：派生 kind 灰显类名 / 相对时间格式化（2）
3. `MemoryActions`：忘记全部两段式（点击 → armed → 再点触发 / 超时回弹）（2）

## 4. 验收清单

> 2026-11-20 真机验收通过。

- [x] 记忆页与 SQLite 实况一致（抽 3 条对库核对）
- [x] 整理记忆按钮跑通并刷新；忘记全部两段式 → 库清空 → 下次启动弹 onboarding
- [x] 派生事件灰显可辨识；预测命中率与 predict.rs 统计一致
- [x] 空库（新装）不报错、不出现 NaN

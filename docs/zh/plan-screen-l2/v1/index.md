# 方案：P2-4 屏幕感知 L2（窗口标题白名单 + 教名词）

| 文档信息 | 内容 |
|---|---|
| 版本 | v1 |
| 日期 | 2026-11-24 |
| 状态 | **已实施**（代码；真机 Screen Recording / 标题可用性待勾） |
| 上游 | [`roadmap-agent-capabilities.md`](../../roadmap-agent-capabilities.md) P2-4 / G-02 |
| 定位约束 | 不截屏、不做视觉理解；标题仅白名单 App；原文不出机器；默认关 |
| 一句话 | **只对用户勾选的 App 记窗口粗标签；可用「教名词」把标题收成短词；未勾选 = 事件里没有窗口信息** |

---

## 0. 范围

| 做 | 不做 |
|---|---|
| `window_title` 子开关（默认关）+ `title_allowlist` | 截屏 / OCR / 多模态理解 |
| 白名单命中时落 `window: App \| label` | 全量 App 抓标题 |
| `taught_nouns`：标题含 needle → 存 label | 自动从标题抽实体进 facts |
| 前台 App 名仍走既有 `app:`（`active_app`） | 改采样间隔 / idle 语义 |
| perception_search 增加 window 聚合 | 实时标题塞进每轮 system（仍靠事件查询） |

---

## 1. 不变量

1. **未勾选无窗口信息**：`window_title=0` 或 App ∉ allowlist → 不写 `window:` 行（可有 `app:`）。
2. **开关在采集层**：关了就不采样标题字段进事件。
3. **粗标签**：无命中教名词时标题截断（≤80 字、压空白）；命中则只存用户给的 label。
4. **旧配置兼容**：缺字段 serde default → 全关 / 空名单。
5. **不进 facts**：与其它 perception 一样，consolidate 不吃 perception 原文。

---

## 2. 验收

| ID | Given / When / Then |
|---|---|
| S1 | allowlist 含「Cursor」，前台 Cursor 且标题变化 → 一条 `window: Cursor \| …` |
| S2 | 前台 Safari 但不在名单 → 无 `window:`（可有 `app: Safari`） |
| S3 | taught needle=`bonsai` label=`盆栽`，标题含 bonsai → `window: … \| 盆栽` |
| S4 | `window_title=false` → 永不写 `window:` |
| S5 | 旧 `perception.json` 无新字段 → 加载为 default，不炸 |

---

## 3. 变更记录

| 版本 | 日期 | 说明 |
|---|---|---|
| v1 | 2026-11-24 | 初稿并实施：perception 扩展 + PerceptionCard + search 聚合 |

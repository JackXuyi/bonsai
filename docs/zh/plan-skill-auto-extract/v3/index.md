# 自动提取 Skill · M3 质量闭环

| 文档信息 | 内容 |
|---|---|
| 版本 | v3 |
| 日期 | 2026-11-25 |
| 状态 | **已实施**（淘汰 + 使用反馈；云端增强仍不做） |
| 上游 | [`../../plan-skill-auto-extract.md`](../../plan-skill-auto-extract.md) M0–M2；AGENTS §12（`skill_auto/` 目录拆分） |
| 一句话 | **未启用草稿超时删除 + 启用后注入记次数；仍不自动启用、不云端增强生成** |

---

## 相对 M2 的增量

| 做 | 不做 |
|---|---|
| `AutoMetaEntry.use_count` / `last_used_at` | 云端专用生成模型 |
| kit 挂载自动包时 `record_skill_use` | 自动启用草稿 |
| `gc_stale_drafts`：未启用且创建 ≥14 天 → 删包 + meta | 按使用率自动停用已启用包 |
| 夜间 `run_if_due` 尾调用 GC；命令 `skill_auto_prune_drafts` | SCHEMA bump |

## 行为

1. **使用反馈**：`build_agent_kit` 注入启用中的 auto skill 时 +1 `use_count`。
2. **淘汰**：仅针对 **meta 中存在且当前未启用** 的包；用户手写 skill 不受影响。
3. **UI**：草稿行可点「清理过期草稿」；已启用自动包展示「已用 N 次」。

## 变更记录

| 版本 | 日期 | 说明 |
|---|---|---|
| v3 | 2026-11-25 | M3 最小闭环 + `skill_auto/` 拆分 |

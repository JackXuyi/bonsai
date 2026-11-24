# 方案：P4-3 外部内容安全 + 日时间线导出

| 文档信息 | 内容 |
|---|---|
| 版本 | v1 |
| 日期 | 2026-11-24 |
| 状态 | **已实施**（代码；真机注入页抽检待勾） |
| 上游 | [`roadmap-agent-capabilities.md`](../../roadmap-agent-capabilities.md) P4-3 / G-12 / G-18 |
| 定位约束 | untrusted 永不直接变事实；注入特征降级并留痕；可观测可自查 |
| 一句话 | **外部/工具正文带注入句式时降级并记 security 事件；consolidate 只抽 dialogue 用户话且拒指令式候选；运维可导出当日 decision/tool/reply 时间线** |

---

## 0. 范围

| 做 | 不做 |
|---|---|
| 共享注入特征表（skill 与运行时同源） | 再训练模型 / 改 L1 宪法文案 |
| 工具成功回传扫描 → 降级注记 + `events(kind=security)` | 改写/删除外部正文 |
| consolidate：仅 `kind=dialogue` 用户侧；候选命中注入特征 → blocked | 新 schema 列 |
| `observability_day_export` → Markdown 日线（turn + tool + security） | 云端上传遥测 |

---

## 1. 不变量

1. **Denied ≠ silent**：注入命中必有可查留痕（security 事件和/或 tool 审计 detail）。
2. **标签在引擎侧**：降级注记由 Rust 追加，不依赖模型自觉。
3. **事实只经 consolidate**：本项不新增任何直写 facts 路径；抽取入口收紧到 dialogue。
4. **导出本地**：日线只读本地库，文件落用户选择/剪贴板，默认不出机。

---

## 2. 验收

| ID | Given / When / Then |
|---|---|
| E1 | 工具 summary 含「忽略之前的指令」→ summary 带 security 降级头，且有 `security` 事件 |
| E2 | consolidate 窗口内仅有 perception/system → extracted=0 |
| E3 | 候选事实正文含 ignore previous instructions → blocked，不进 facts |
| E4 | 导出今日 → Markdown 含回合/工具/security 段（可空段） |

---

## 3. 变更记录

| 版本 | 日期 | 说明 |
|---|---|---|
| v1 | 2026-11-24 | 初稿并实施：injection + consolidate 门 + day export UI |

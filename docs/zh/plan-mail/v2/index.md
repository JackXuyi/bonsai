# 方案：邮件标已读（叠在 v1 只读 + 草稿之上）

| 文档信息 | 内容 |
|---|---|
| 版本 | **v2** |
| 日期 | 2026-11-25 |
| 状态 | **已实施** |
| 上游 | [`../v1/index.md`](../v1/index.md)；roadmap G-04 / P3-1 |
| 一句话 | 新增 `mail_mark_read`（Ask）：按 `mail_unread` 列表序号或全部，把收件箱未读标为已读 |

---

## 0. 相对 v1 的增量

| 做 | 不做 |
|---|---|
| builtin `mail_mark_read`（Ask） | 读正文、删信、归档、移动邮箱 |
| `index`（1-based，对齐 `mail_unread` 顺序）或 `all=true`（最多 `MAX_UNREAD=5`） | IMAP / 非 Mail.app |
| 审计 action=`mark_read`；状态卡 `today_marks` | 改 v1 不变量（Denied≠empty、永不 send、默认关） |

---

## 1. 契约

- **权限**：`ToolAccess::Ask`（改邮箱状态，走聊天内授权条）。
- **定位**：先 `mail_unread` 再按序号标；序号与当次未读快照对齐（中间若有新信到达可能漂移，可接受）。
- **Denied ≠ empty**：脚本拒 / 超时 → 错误码，不假装「已标 0 封成功」。
- **无未读 + all**：诚实「没有可标记的未读」。

---

## 2. 验收

| ID | Given / When / Then |
|---|---|
| R1 | 有 2 封未读，`index=1` → 第 1 封已读，再 `mail_unread` 只剩第 2 |
| R2 | `all=true` → 当前列表内未读全部已读 |
| R3 | 缺 `index` 且非 `all` → 参数错误 |
| R4 | 开关关 → kit 无 `mail_mark_read`（随 mail provider） |

---

## 3. 变更记录

| 版本 | 日期 | 说明 |
|---|---|---|
| v2 | 2026-11-25 | `mail_mark_read` + Mock/AppleScript + 单测 |

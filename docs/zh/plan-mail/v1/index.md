# 方案：P3-1 邮件只读 + 摘要（实验 / Mail.app）

| 文档信息 | 内容 |
|---|---|
| 版本 | v1 |
| 日期 | 2026-11-24 |
| 状态 | **已实施**（按推荐默认；真机 Automation 授权待勾） |
| 后续 | 规则腿退役 / 仅云端 FC 见 [plan-mail-cloud-fc/v1](../../plan-mail-cloud-fc/v1/index.md)；标已读见 [`../v2/index.md`](../v2/index.md) |
| 上游 | [`roadmap-agent-capabilities.md`](../../roadmap-agent-capabilities.md) P3-1 / G-04；[`plan-calendar.md`](../../plan-calendar.md) §3.9 |
| 定位约束 | 只读优先；Denied ≠ empty；原文/主题不落长期记忆；发送永不执行，仅草稿 |
| 一句话 | **未读发件人+主题可查可摘要；关掉就看不见；绝不用空列表假装没邮件** |

---

## 0. 范围

| 做 | 不做 |
|---|---|
| Mail.app AppleScript 只读最近 N 封未读（发件人 + 主题） | IMAP / 账号密码 / Spark·Outlook |
| `tools.mail.enabled` 默认关；工具页实验卡 | 读正文、附件、全量同步 |
| builtin：`mail_unread`（Read）、`mail_draft`（Ask，只建草稿不发送） | `send` 任何路径 |
| 摘要进当次工具结果 / 本地规则答复；**不**写 facts/events | 云端默读邮件（永不出机器：摘要可进云端 tool 结果，但源数据本机拉取） |
| 失败码：`mail_denied` / `mail_unavailable` / `mail_timeout` | 空列表伪装「没有未读」 |

---

## 1. 不变量

1. **Denied ≠ empty**：Automation 拒 / Mail 未开 / 超时 → 明确错误码，不返回 `[]`。
2. **开关在挂载层**：`tools.mail.enabled=0` 时 provider 不进 kit。
3. **最高敏**：主题/发件人只进当次 tool summary；consolidate / extract **不得**消费 mail 审计正文（审计只记计数）。
4. **草稿 ≠ 发送**：`mail_draft` 只 `make new outgoing message` + visible；代码路径无 `send`。
5. **本地零 tools[]**：本地 chat 不挂 mail 工具；规则腿可代答未读摘要（引擎侧，对齐日历 `tools.rs`）。

---

## 2. 验收

| ID | Given / When / Then |
|---|---|
| M1 | 未授权 / 脚本拒 → `mail_denied`（或 unavailable），非空列表 |
| M2 | 授权且无未读 → 诚实「没有未读」，非错误 |
| M3 | 开关关 → kit 无 `mail_*` |
| M4 | `mail_draft` 无 send 分支（单测锁源码/行为） |
| M5 | 主题不进 `facts` / `events`（工具结果不落库为记忆） |

---

## 3. 变更记录

| 版本 | 日期 | 说明 |
|---|---|---|
| v1 | 2026-11-24 | 初稿并实施：mail.rs + MailCard + kit 挂载 |

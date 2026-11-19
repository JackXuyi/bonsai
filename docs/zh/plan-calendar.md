# 方案：日历 / 邮件工具调用（MVP P2 第 2 项）

> **状态**：待评审
> **日期**：2026-11-19
> **依赖**：决策层 `should_speak`（decision.rs，已落地，含 `in_meeting` 静默）、感知层轮询与逐项授权模式（perception.rs，已落地）、settings KV 与 events 审计链（memory/db.rs，已落地）、日报生成器（report.rs，已落地）、本地 LLM（llm.rs，已落地，可降级）
> **对应调研**：docs/zh/research-report.md §3.3 macOS TCC 四条约束、§6.4 打断成本与「会议中绝不开口」、§8.2 分层架构、§8.3 P2 第 2 项
> **一句话**：让阿盆真的知道「你 3 点有个会」，并**只在这件事上**被允许开口。

---

## 1. 目标与非目标

### 目标

1. **第一件真活**：会说「14:00 产品评审，还有 25 分钟」，而不是只会聊今天剪贴板出现了什么。这是从「陪聊」到「搭子」的分界线。
2. **会前提醒成为 L4 的第二个出口**，且是**唯一一个不受「每天 1 次」额度约束**的出口——理由见 §3.6，代价是它自带硬上限。
3. **会议中彻底闭嘴**：把 `perception.rs` 里那行 `in_meeting: false, // P1: calendar leg` 真正接上。调研 §6.4 把「会议中不打断」列为硬约束，而当前代码永远认为你不在会里。
4. **只读优先，且只读就够**：P2-2 全期不做写操作（建日程/发邮件），只预留接口与开关键位。理由见 §1 非目标。
5. **权限失败必须可见**：没授权 ≠ 今天没会。宁可显式报「没权限」，也绝不静默返回空列表——这是本方案最重要的一条不变量。

### 非目标（本期明确不做）

| 不做 | 理由 |
| --- | --- |
| **写操作**（建日程、改时间、发邮件） | 失败模式远贵于收益：时间冲突、时区错、重复创建、撤销困难，每一项都要「预演 → 确认 → 可撤销」三层 UI。P2 预算不够，且只读已经能兑现「会前提醒 + 日报日程」两个价值点。预留 `ToolIntent::CreateEvent` 枚举位与 `tools.calendar.write` 开关键位，不实现 |
| **日历数据进预测循环** | 预测循环吃的是「你自己养出来的节律」。把日程喂进去，「猜你在做什么」就退化成「读你的日程表」——既是作弊，也是隐私越界。`predictions.basis` 永远只有 preference/event 两类，不新增 calendar |
| **读历史日程** | 只读「今天 00:00 → 明天 23:59」。历史日程对「接下来要干嘛」零价值，只增加泄露面 |
| **邮件全量同步 / IMAP 账号** | 要账号密码，等于把邮箱主钥匙交给一个桌宠。邮件见 §3.9，拆成独立的 T5，且只做「当前未读摘要」 |
| 云端模型读日程 | 日程只喂本地模型。`llm.rs` 若将来接云端增强，日历/邮件必须在「永不出机器」白名单里硬编码排除 |

---

## 2. 现状盘点（写方案前的代码事实）

| 已有 | 位置 | 对本方案的意义 |
| --- | --- | --- |
| `decision::should_speak` 已把 `in_meeting` 当**绝对静默**（先于额度判断） | decision.rs:70 | 日历只需提供 `in_meeting`，静默逻辑零改动 |
| `decision_input` 里 `in_meeting: false, // P1: calendar leg` | perception.rs:321 | 这个 TODO 就是本方案的正式接口 |
| 每日主动额度 `DEFAULT_DAILY_LIMIT = 1` | decision.rs:56 | 会前提醒不能挤这条额度，否则一天 3 个会只能提醒 1 次（§3.6） |
| 感知层「总开关 + 逐源开关 + 默认全关」模式 | perception.rs:28–68 | 日历直接复用这套心智模型，不发明新交互 |
| `settings` KV 表（upsert） | db.rs | 工具开关落这里，不新建配置表 |
| `events` 表 + `report::collect` 按前缀分流 | report.rs:51–64 | 日报要加日程节，沿用「前缀分流」写法 |
| 人格渲染器 `PromptContext` | persona/prompt.rs | 回答日程时的语气复用同一套，不另起模板引擎 |
| **无 Info.plist**（`src-tauri/` 下不存在） | — | TCC 授权弹窗不会出现，必须先补 usage description（§3.3） |
| **无 EventKit 依赖** | Cargo.toml | 需引入 `objc2-event-kit`（§3.3 已核实版本） |
| **无任何权限状态的地方** | — | 权限四态是本方案新增的一等公民，不是错误分支 |

---

## 3. 核心机制设计

### 3.1 能力分层：只读三档，写操作只留接口

| 档位 | 内容 | 默认 | 授权 | 打扰 |
| --- | --- | --- | --- | --- |
| **R0 日程上下文** | 当前/下一个会议的时间，供 `in_meeting` 判断 | 关（随总开关） | 系统日历权限 | 从不主动说 |
| **R1 会话内查询** | 「今天有什么安排」「下一个会是什么」 | 同上 | 同上 | 只在用户问时说 |
| **R2 会前提醒** | 会前 10 分钟主动提一句 | 开启日历后默认开 | 同上 | **独立 lane，日上限 3** |
| ~~W1 写~~ | 建/改日程 | — | — | 本期不实现 |

**授权时机是本方案最关键的产品决策**：日历开关**默认关**，而且**不塞进首次引导**。首次启动就弹日历授权，用户 90% 会点「不允许」，而 macOS 的拒绝是**粘性**的——再想授权只能去系统设置手动改。所以：用户在工具页主动把日历开关拨开时，才触发系统授权弹窗（用户主动触发是 TCC 通过率最高的时机）。

### 3.2 Schema v7：两张表（缓存 / 审计）

```sql
-- 日历只读缓存：TTL 5 分钟，范围仅「今天 + 明天」。
-- 为什么缓存：EventKit 的 events(matching:) 在大日历上可能几百毫秒，
-- 而 in_meeting 判断每 15 秒要读一次。缓存把「查询成本」与「判断频率」解耦。
CREATE TABLE IF NOT EXISTS calendar_cache (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    external_id    TEXT NOT NULL,        -- EKEvent.eventIdentifier
    start_at       TEXT NOT NULL,        -- RFC3339 UTC（与 events.ts 同一时钟口径）
    end_at         TEXT NOT NULL,
    title          TEXT NOT NULL,        -- 只在缓存里，TTL 到期即弃，**不进 events / facts**
    calendar_name  TEXT,
    all_day        INTEGER NOT NULL DEFAULT 0,
    attendee_count INTEGER NOT NULL DEFAULT 0,
    declined       INTEGER NOT NULL DEFAULT 0,  -- 自己拒掉的会
    canceled       INTEGER NOT NULL DEFAULT 0,
    availability   TEXT,                 -- 'busy' | 'free' | 'tentative' | 'unavailable'
    reminded_at    TEXT,                 -- 提醒幂等：一次会只提醒一次
    fetched_at     TEXT NOT NULL,
    -- 重复日程的每个实例 eventIdentifier 常与主事件相同，必须带开始时间才唯一
    UNIQUE(external_id, start_at)
);

-- 「它做了什么」审计：只有动作与结果，**没有标题、没有正文**。
CREATE TABLE IF NOT EXISTS tool_calls (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    ts          TEXT NOT NULL,
    tool        TEXT NOT NULL,   -- 'calendar' | 'mail'
    action      TEXT NOT NULL,   -- 'agenda' | 'next' | 'search' | 'remind' | 'digest'
    outcome     TEXT NOT NULL,   -- 'ok' | 'denied' | 'unavailable' | 'error'
    detail      TEXT,            -- 只放计数/耗时，例如 '3 events, 42ms'
    duration_ms INTEGER
);
```

写入语义：
- `calendar_cache` 是**纯缓存**，可随时整表 DELETE（工具页「清空日历缓存」按钮、关开关、`forget_all` 级联、授权被撤销）。
- `tool_calls` 是审计，`forget_all` 级联；工具页按天展示最近 20 条，用户能看到「它今天读了 3 次日历、提醒了 1 次」，但看不到任何会议标题——因为它压根没被记进去。
- `settings` 新增键位（复用现有 KV，不新建表）：`tools.calendar.enabled`、`tools.calendar.remind`（默认 1）、`tools.mail.enabled`（默认 0）、`tools.calendar.write`（预留，恒 0）。

### 3.3 EventKit 桥：必须在主进程内调用

**依赖选型（已核实）**：`objc2-event-kit 0.3.2`（crates.io，madsmtm/objc2 生态，32 万次下载，2025-10 更新），依赖 `objc2 >= 0.6.2,<0.8.0` + `block2 >= 0.6.1`。比自写 FFI 稳，比 `highlandcows-eventkit` 这种单人维护的薄封装风险低。

```toml
[target.'cfg(target_os = "macos")'.dependencies]
objc2 = "0.6"
objc2-foundation = { version = "0.3", features = ["NSString", "NSArray", "NSDate"] }
objc2-event-kit = "0.3"
block2 = "0.6"
```

**四条 TCC 事实（调研 §3.3 原文推导，直接决定实现方式）**：

1. **必须在主 App 进程内调用**。不能用 Swift helper 或 sidecar 代读——TCC 权限归属「责任进程」，helper 自己是个新 App，会各自弹各自的授权框，用户会看到两次莫名其妙的面板，且主 App 依然没权限。
2. **授权弹窗必须从主线程发起**。Tauri 的命令处理跑在 worker 线程上，`requestFullAccessToEvents` 直接调用不会弹窗（只会静默返回 denied）。实现：`app.run_on_main_thread(|| store.requestFullAccessToEvents(...))` + 条件变量等待，**超时 60s**（用户可能把弹窗丢在一边，不能把命令线程永久挂住）。
3. **必须区分 `writeOnly`**。macOS 14 起 `authorizationStatus` 有五态：`notDetermined / restricted / denied / fullAccess / writeOnly`。`writeOnly` 是「只能添加日程」——很多实现只看 `granted: bool` 就当成成功了，结果读出来永远是空数组。**判断条件必须是 `fullAccess`**，这是本方案明确的必失败反例来源。
4. **签名必须稳定**。ad-hoc 签名每次构建都算新 App，之前授的权限全部失效，表现为「昨天还好好的，今天读不到日程」。验收清单里必须有这一条：`tauri build` 出的 .app 用固定签名身份（自签稳定证书或 Developer ID），并且**开发期验收必须在 .app 里做**——`tauri dev` 跑的是 `target/debug/bonsai` 裸二进制，与 bundle 的 TCC 身份不同。

**Info.plist**：`src-tauri/Info.plist`（Tauri 2 会自动合并进 bundle）需补三条：

```xml
<key>NSCalendarsFullAccessUsageDescription</key>
<string>阿盆读你的日程，只为了在你开会前提醒一句、开会时不打扰你。日程不会离开这台电脑。</string>
<key>NSCalendarsUsageDescription</key>
<string>阿盆读你的日程，只为了在你开会前提醒一句、开会时不打扰你。</string>
<key>NSCalendarsWriteOnlyAccessUsageDescription</key>
<string>阿盆不会创建或修改日程。</string>
```

（第三条是为了在系统设置里显示正确的说明；App Sandbox 默认未开，故不需要 `com.apple.security.personal-information.calendars` 授权文件。若将来开沙盒需一并加。）

**抽象层**（沿用仓库既有的 trait 双轨模式，与 `FactExtractor` / `PredictionGenerator` / `DialogueBackend` 一致）：

```rust
pub enum CalendarAccess { NotDetermined, Denied, Restricted, WriteOnly, Full }

pub enum CalendarError { Access(CalendarAccess), Unavailable(String), Timeout }

pub struct EventItem {
    pub external_id: String,
    pub start: chrono::DateTime<Utc>,
    pub end: chrono::DateTime<Utc>,
    pub title: String,
    pub calendar_name: Option<String>,
    pub all_day: bool,
    pub attendee_count: usize,
    pub declined: bool,
    pub canceled: bool,
    pub availability: String,
}

/// 只读，两个方法。写操作不在 trait 里，避免「顺手加个 create」。
pub trait CalendarSource: Send {
    fn access(&self) -> Result<CalendarAccess, CalendarError>;
    fn request_access(&self) -> Result<CalendarAccess, CalendarError>;
    fn events_between(&self, start: DateTime<Utc>, end: DateTime<Utc>) -> Result<Vec<EventItem>, CalendarError>;
}
```

- `EkCalendarSource`：macOS 真实实现（`cfg(target_os = "macos")`）
- `MockCalendar`：测试 + 非 macOS 平台的空实现（返回 `Unavailable`，保证跨平台 `cargo check` 与 CI 全绿）
- 失败一律**向上冒泡成 `CalendarError`**，绝不 `unwrap_or_default()` 吞成空列表——这是 §1 那条不变量的代码形态。

### 3.4 权限状态机与降级（拒绝 ≠ 没有会）

```
NotDetermined ──用户拨开关──▶ [系统弹窗] ──允许──▶ Full      ──▶ 全部能力
                                   └──拒绝──▶ Denied          ──▶ 显式「未授权」+ 跳系统设置
Denied ──用户去系统设置打开──▶（下次查询）Full
Full ──用户在系统设置撤销──▶ Denied  ──▶ 自动清 calendar_cache，保留开关（用户可能马上改回来）
```

降级行为表（这是给实现看的死规定）：

| 状态 | 命令返回 | 前端显示 | 会前提醒 | `in_meeting` |
| --- | --- | --- | --- | --- |
| `Full` | 正常数据 | 正常 | 开（若开关开） | 真实判定 |
| `Denied` / `Restricted` | `Err("calendar_denied")` | 「日历权限未授予」+ 按钮跳 `x-apple.systempreferences:com.apple.preference.security?Privacy_Calendars` | 停 | **false**（无声降级，绝不因此误判静默） |
| `WriteOnly` | `Err("calendar_write_only")` | 「只给了添加权限，读不了日程」+ 同上按钮 | 停 | false |
| `NotDetermined` | `Err("calendar_not_determined")` | 「还没授权」+「去授权」按钮 | 停 | false |
| 非 macOS / EventKit 异常 | `Err("calendar_unavailable")` | 「这台机器不支持」 | 停 | false |

注意 `in_meeting` 在无权限时必须回落 `false`：错的静默比错的开口伤害小，但**静默到永远不说话同样是 bug**，所以状态必须显示在前端，不能藏着。

### 3.5 Intent 路由：规则优先，模型可选

工具调用不做「模型 function calling 优先」。理由：本地 2B 模型的 function calling 稳定性不足，一旦误判就会把「我今天好累」当成「查今天日程」，这是最伤人的失败。所以：

**第一层：规则路由（必须能独立工作，模型挂了也照样查日程）**

```rust
pub enum ToolIntent {
    AgendaToday,
    AgendaTomorrow,
    NextEvent,
    SearchEvent { query: String },
    MailDigest,          // T5 才接
    None,                // 交回普通对话，不接管
}
```

规则设计（防误吞是关键）：
- **必须同时命中「时间词」+「日程名词」**才算意图。「今天」单独出现不算。
  - ✅「今天有什么**安排**」「**明天**有**会**吗」「**下一个会**是什么时候」「**日程**里有没有和 X 有关的」
  - ❌「我今天好累」「明天天气怎么样」「这个会**议**记录帮我整理一下」（原型是「会议」但不是查日程——需要 phrase 级匹配，不是子串匹配）
- 匹配用有序短语表（长的优先），不用正则堆叠：`["今天有什么安排","今天的日程","今天有几个会","明天有什么安排","下一个会","接下来有什么会","还有多久开会"]`。
- 未命中 → `ToolIntent::None`，走原有对话管线，零行为变化。

**第二层：模型改写（可选增强）**
路由判定命中后，把结构化的 `Vec<EventItem>` 交给本地模型润色成阿盆的语气（如「14:00 产品评审，还有 25 分钟。」），**但事实只能来自结构体，模型只做措辞**。模型挂 / 超时 / 输出里出现结构体之外的会议名 → 直接用确定性模板：

```
今天 2 个会：14:00 产品评审、16:30 一对一。
下一个：14:00 产品评审，还有 25 分钟。
明天没有安排。
```

模板永远可用，这就是「不依赖真模型」的落点。

### 3.6 会前提醒：L4 的第二条 lane

**问题**：`DEFAULT_DAILY_LIMIT = 1`。如果提醒挤这条额度，一天三个会只能提醒一个，功能等于废掉。

**判断**：每日 1 次额度是用来约束**开放式主动搭话**的——那种「价值不确定、可能惹人烦」的开口。会前提醒是另一类：**时间确定、用户显式开启、漏了就是失职**。把它和「我觉得你今天挺累的」放进同一个额度池，是把两类完全不同的东西混为一谈。

**方案**：新增 `Lane::TimeSensitive`，与 `Lane::Proactive` 平行，但自带四道硬闸：

| 闸门 | 规则 | 理由 |
| --- | --- | --- |
| 窗口 | 只在 `[start-12min, start-8min]` 内触发（目标 −10min，容差 ±2） | 太早用户已经知道，太晚来不及 |
| 日上限 | ≤ 3 条 / 自然日 | 超过 3 个会的一天，提醒变成噪音 |
| 间隔 | 相邻提醒 ≥ 10 分钟 | 背靠背的会不能连播两条 |
| 静默 | 22:00–08:00 不提；会议进行中不提；全天事件 / 已拒绝 / 已取消 / `availability=free` 且无参会人 不提 | 深夜话少是 persona 的既有性格；会中开口是最严重的打断 |

另外两条：
- **用户点「别烦我」= 当天提醒全关**（写 `settings.tools.calendar.remind_muted_date`）。这与预测循环的 `shut_up` 语义一致：行使控制权不该被惩罚，但必须被尊重。
- 提醒文案**不占聊天流以外的通道**：走与形态线播报同一条气泡通路（`announcement` 样式），不弹系统通知——P2 不做通知中心，App 不在前台就只更新托盘 tooltip，不打扰。

调度实现：复用 `morph.rs` 的 60s 轮询形态（`spawn` + `sleep(60)` + `last_tick` 幂等），但独立的 loop 与独立的 `last_remind_at`，避免两条线互相卡住。

### 3.7 `in_meeting` 接线（那个 P1 TODO）

```rust
// perception.rs::decision_input 增加入参
pub fn decision_input(signals: &LatestSignals, signals_today: usize, in_meeting: bool) -> DecisionInput {
    DecisionInput { in_meeting, .. }
}
```

判定：`now ∈ [start, end)` 且 `attendee_count >= 2` 且 `!declined && !canceled`。
- **为什么要求参会人 ≥ 2**：自己建的「专注时间」「写周报」这类单人日程不该让阿盆闭嘴——那恰恰是最该被搭话的时段。
- 判定读的是 5 分钟 TTL 的缓存，不每 15 秒打 EventKit。
- 无权限 → `in_meeting = false`（见 §3.4 降级表），并在工具页显示「会议静默未生效：日历未授权」。这条提示不能省：用户以为开了静默但实际没开，比没有这个功能更糟。

### 3.8 日报加一节「明天」

日报是晚上生成的，说今天已经过期。在 `report::render` 末尾（一句话总结之前）加：

```markdown
## 明天

- 09:30 产品评审（3 人）
- 14:00 一对一
```

规则：
- 只列明天的（`tomorrow 00:00 → 23:59`），最多 5 条，超过就「等共 8 个，前 5 个：」
- 全天事件标注「（全天）」；无权限时输出「（日历未授权，看不到明天安排）」，**不是静默省略**
- 摘要 prompt 里加一句明天的日程，让一句话总结能自然带上「明天上午有个会」

### 3.9 邮件：为什么拆出去

**技术现实**：Apple Mail 没有公开 API。唯一可行的只读路径是 AppleScript（`tell application "Mail" to get subject of messages of inbox whose read status is false`），代价是：
- 需要 **Automation（Apple Events）权限**——又是一次系统弹窗，且这个权限在 macOS 上比日历更容易被拒
- **Mail 必须在运行**，否则超时（用户用 Spark / Outlook 就完全无效）
- 脚本执行慢（几百毫秒到几秒），且邮件多了会卡
- 结果极度依赖 Mail 的 AppleScript 字典版本

**结论**：邮件拆成 **T5，独立验收**，不进本期主链路。做的话也只做最小形态：
- 只读「最近 5 封未读」的**发件人 + 主题**，不读正文、不读附件
- 需要用户在工具页单独拨开，先在界面上写清「需要 Mail 正在运行、会请求自动化权限」
- 失败（Mail 没开 / 权限拒 / 脚本超时）一律返回 `MailError`，降级到「邮件摘要不可用」，**绝不用空列表假装「没有未读」**
- 摘要只进当次会话气泡，**不落 events、不进 facts**（邮件正文是最高敏数据，主题也不该进长期记忆）

### 3.10 前端（克制，与感知层同构）

工具页新增一张卡，结构与「感知层」卡完全一致（用户不需要学第二套交互）：

```
┌ 日历 ──────────────────────────────── [总开关] ┐
│ 授权状态：已授权 / 未授权（按钮：去系统设置）      │
│ ── 会前提醒 ─────────────────────────  [开关]   │
│    会前 10 分钟说一句，每天最多 3 次              │
│ ── 写入 ───────────────────────────  （灰）不提供 │
│ 它今天读了 3 次日历 · 提醒 1 次   [清空日历缓存]  │
└──────────────────────────────────────────────┘
┌ 邮件（实验）───────────────────────── [总开关] ┐
│ 需要 Mail 正在运行，会请求自动化权限               │
└──────────────────────────────────────────────┘
```

- 「它今天读了 N 次」读 `tool_calls` 聚合，纯计数无内容
- 「清空日历缓存」= `DELETE FROM calendar_cache`，一键即清，不需要二次确认（缓存本身就是可弃数据）
- 不做独立的「日程面板」。理由与关系线相同：一有面板，用户就会盯着看，产品就从「搭子」变成「日程 App」

---

## 4. 数据流总览

```
                  用户拨开日历开关
                          │
                ┌─────────▼──────────┐
                │ EventKit 主线程弹窗 │  ← Info.plist usage description
                └─────────┬──────────┘
              Full ───────┴─────── Denied/WriteOnly
                │                      │
     ┌──────────▼──────────┐   前端显示「未授权」+ 跳系统设置
     │ 60s 拉取（今天+明天）│   命令返回 Err，绝不返回空列表
     │ → calendar_cache    │
     └──────────┬──────────┘
                │
     ┌──────────┼───────────────┬─────────────────┐
     ▼          ▼               ▼                 ▼
 in_meeting  会话查询        会前提醒 lane      日报「明天」节
 (15s 判定)  (规则路由→模板/LLM) (10±2min, ≤3/天)  (最多 5 条)
     │          │               │
     ▼          ▼               ▼
 L4 静默   聊天气泡       播报气泡（不弹系统通知）
                │               │
                └───────┬───────┘
                        ▼
                  tool_calls 审计（只有动作与计数）
```

---

## 5. 里程碑

| 里程碑 | 内容 | 规模 | 依赖 | 风险 |
| --- | --- | --- | --- | --- |
| **T0 权限 spike** | `objc2-event-kit` 编译通过；`src-tauri/Info.plist` 合并生效；`.app` 里能弹出授权框；四态读取得对；`writeOnly` 不被误判；固定签名后重启不丢权限 | 0.5 周 | 无 | **最高风险项**，单独做、单独验收，不通过则整个方案改道（退回「只读剪贴板里的日程文本」这种降级形态） |
| **T1 schema v7** | `calendar_cache` + `tool_calls` + settings 键位 + `forget_all` 级联 + 清缓存命令 | 0.5 周 | 无 | 低 |
| **T2 EventKit 桥** | `CalendarSource` trait + `EkCalendarSource` + `MockCalendar` + 权限状态机 + 五条降级路径 + 跨平台编译 | 1 周 | T0, T1 | 中（主线程等待/超时） |
| **T3 意图路由** | 规则路由 + 四个只读意图 + 模板回答 + 模型润色（可降级）+ 35 条样例集 CI | 1 周 | T2 | 中（误吞普通对话） |
| **T4 提醒与接线** | 会前提醒 lane（四道闸）+ `in_meeting` 接入 L4 + 日报「明天」节 + 工具页卡片 + 四指标 CI | 1 周 | T3 | 中（变成通知中心） |
| **T5 邮件（可选）** | AppleScript 只读未读摘要 + Automation 权限引导 + 失败降级 | 1 周 | T4 | 高（Mail 不在跑就废） |

**合计 4 周（不含 T5）**。比 P2 概览里估的 2–3 周大，差在两块原估没算：T0 的 TCC 摸索（调研 §3.3 四条约束没有一条能跳过）和 T4 的 L4 扩展（原来以为是「接个字段」，实际要新增一条 lane 与四道闸）。

T0–T1 不依赖真模型、也不依赖系统授权，可以立即开工。

---

## 6. 指标（常驻 CI，各含必失败反例）

| 指标 | 定义 | 阈值 | 必失败反例（测试必须红） |
| --- | --- | --- | --- |
| **降级正确率** | 权限非 `Full` 时返回 `Err` 的比例 | 100% | 把 `Denied` 返回成 `Ok(vec![])`；把 `WriteOnly` 当成已授权 |
| **提醒窗口正确率** | 提醒落在 `[start-12, start-8]` 且每会仅一次 | 100% | 会前 20 分钟触发；同一会提醒两次；全天事件触发 |
| **限流正确率** | 日 ≤3、间隔 ≥10min、22–08 静默、会中不提 | 100% | 第 4 个会仍提醒；两个背靠背的会连播；23:30 提醒明早 9 点的会 |
| **意图不误吞率** | 35 条样例（20 正 / 15 负，「我今天好累」「明天天气」必须走 `None`） | 100% | 「我今天好累」被判成 `AgendaToday` |
| **静默不误判率** | 单人日程（专注时间）不得让 `in_meeting` 为 true | 100% | `attendee_count` 不校验导致自己写周报时全程闭嘴 |

---

## 7. 开放问题（待评审，均给了建议默认值）

1. **会前提醒提前多久？** 建议 10 分钟，容差 ±2。理由：够走到会议室，又不至于「还有 10 分钟」时用户早就知道了。
2. **日历总开关默认值？** 建议**关**，且**不塞进首次引导**，只在工具页用户主动拨开。（理由见 §3.1，TCC 拒绝是粘性的）
3. **会议标题是否进 `events` / 长期记忆？** 建议**不进**。标题只活在 TTL 5 分钟的缓存里，`events` 只记「读了 3 次日历」这类计数。代价是日报的「明天」节不落盘（日报是当场生成的，够用）。
4. **邮件本期做不做？** 建议**不做**，拆 T5。等日历端到端验收完再评估——如果日历的权限转化率本身就低，邮件更没戏。
5. **只读范围？** 建议「今天 00:00 → 明天 23:59」，不读历史、不读下周。

---

## 8. 合规与隐私对照

| 调研要求（§3.3 / §8.2） | 本方案的落点 |
| --- | --- |
| 逐项授权、默认关闭 | 日历/邮件各自独立开关，默认全关；总开关语义与感知层一致 |
| 原文不出机器 | 日程只喂本地模型；缓存 TTL 5 分钟；`events`/`facts` 里没有标题；云端增强白名单硬排除 |
| 权限归属责任进程 | EventKit 在主 App 进程内调用，不用 helper/sidecar（否则 TCC 会分裂成两个身份） |
| 签名稳定 | 验收清单要求固定签名身份 + 在 `.app` 内验收，不用 `tauri dev` 裸二进制 |
| 授权后需重启进程 | 明确写入 UI 提示：从系统设置改完权限回来，App 会提示「需要重启生效」（macOS 部分权限需重启） |
| 用户可反悔 | 工具页一键清空缓存；关开关即整表删除；`forget_all` 级联；系统设置撤销权限后下次查询自动清缓存 |
| 「它看到了什么」可审计 | `tool_calls` 表 + 工具页最近 20 条；只有动作、结果、耗时，没有内容 |
| 不做通知中心 | 提醒只走应用内气泡；App 不在前台时最多更新托盘 tooltip |

---

## 9. 风险与对策

| 风险 | 后果 | 对策 |
| --- | --- | --- |
| ad-hoc 签名导致权限每次构建失效 | 「昨天还能用，今天读不到」，且现象像 bug | T0 先固定签名；`cargo test` 里加一条 `#[ignore]` 的手工验收脚本说明；README 加「开发期权限」一节 |
| 用户在系统弹窗点「不允许」 | 功能永久不可用，且不可再触发弹窗 | 前端给「去系统设置」深链，并明确写「点过不允许后，只能在这里手动打开」 |
| 弹窗丢在一边不点 | 命令线程挂死，整个 App 卡住 | 60s 超时 + 返回 `Timeout` + 前端提示「授权没完成，可以再试」 |
| 大日历 `events(matching:)` 慢 | 轮询线程被拖住 | 5 分钟缓存 + 只查 2 天窗口 + 读取放在独立线程 |
| 提醒变成第二个通知中心 | 核心体验崩坏（用户装它是为了安静） | 四道硬闸 + 「别烦我」当天全静 + 不做系统通知 |
| 规则路由误吞普通对话 | 「我今天好累」被回一句日程，极其出戏 | 时间词 + 日程名词双命中 + 35 条负样本 CI |
| EventKit 在 CI 机器上不可用 | CI 红 | `MockCalendar` + `cfg` 隔离，macOS 真机测试 `#[ignore]` 手动跑 |
| T5 邮件依赖 Mail.app | 用 Spark/Outlook 的用户完全无效 | 明确标注「实验 / 仅支持 Mail.app」，失败必须显式报错 |

---

## 10. 实施备注

- 依赖新增仅 macOS target：`objc2`、`objc2-foundation`、`objc2-event-kit`、`block2`。非 macOS 走 `MockCalendar`，`cargo check` 保持跨平台通过。
- `src-tauri/Info.plist` 是新增文件，Tauri 2 会自动合并；改完必须重新 `tauri build` 才生效（`tauri dev` 不重新生成 bundle）。
- T0 若证伪（`objc2-event-kit` 在当前 macOS 版本上 `requestFullAccessToEvents` 拿不到 `fullAccess`），降级方案：只读「剪贴板里的日程文本」或直接跳过本项做 P2-4 屏幕感知——不要硬啃 FFI。
- T4 完成后，`perception.rs` 的 `// P1: calendar leg` 注释与 `perception::decision_input` 的签名要一并更新，避免留下第二个 TODO。

# 方案：用户画像（结构化身份事实 + 固定注入）

> **状态**：v0.2 待确认（未实施；v0.2 补充 §6 工具页画像卡 UI 与 M3 分期）
> **日期**：2026-11-23
> **依赖**：facts/preferences 表（已落地）、consolidate 抽取管线（已落地）、onboarding q4 抽取拦截（已落地）、persona/prompt.rs 渲染（已落地）、settings 表（schema v5，已落地）
> **对应裁决**：AGENTS.md「能力不得重复实现」——画像是 memory 系统的一层，不新建平行实现；「一切以 AI 优先」——画像的交付面是对话准确率，UI 只做管理

## 1. 问题

用户问「我叫什么」「我是做什么的」「我平时用什么工具」，阿盆答不上来：

| 环节 | 现状 | 缺口 |
| --- | --- | --- |
| `system_prompt()`（dialogue.rs） | 只注入 boundaries + 关系档位 + 心情 | **facts 一概不进 prompt**——模型对「用户是谁」全盲 |
| `memory_search` | 按 key 召回 facts | 被 `might_recall` 门控：只有用户主动问记忆类问题才触发；「帮我写个简历」这类隐式需要画像的场景全漏 |
| facts 表 | 扁平 content 字符串，无 key | 无法做「新值覆盖旧值」的身份语义（换工作 ≠ 两条并存的事实） |
| consolidate 抽取 | 「我在用 X」→ context fact | 抽到了但没有结构化出口，永远躺平在 facts 里不被消费 |
| onboarding | q3/q5 → preferences | 只有边界和偏好，没问「你是谁」（有意为之，5 问已够重） |

核心判断：**画像 ≠ 事实。** 事实是事件性的（「用户在 11 月 20 日说在用 Cursor」），画像是身份快照（「用户现在用 Cursor」）。前者进 facts 留痕、可被总结线引用；后者是 KV、可覆盖、每轮固定注入。二者语义不同，允许并存，不算重复实现。

## 2. 提取哪些画像字段（按优先级）

### P0 — 每轮注入，立刻提升准确率

| key | 含义 | 价值 | 误报风险 |
| --- | --- | --- | --- |
| `name` | 用户姓名 / 希望被怎么称呼 | 对话称呼自然，是「认识你」的最小单元 | 中——「我们叫它阿盆」这类反例需排除 |
| `occupation` | 职业 / 角色（QA、产品经理…） | 回答的隐式上下文（术语深度、举例方向） | 低 |
| `tools` | 常用工具（Cursor、Ollama、Figma…） | 技术话题不瞎猜工具链；多值用顿号拼接存储 | 低——「我在用」句式已有规则先例 |

### P1 — 有则注入，锦上添花

| key | 含义 | 价值 |
| --- | --- | --- |
| `city` | 所在城市 | 天气/作息/本地话题（env 段已有 timezone，city 是补充不是重复） |
| `projects` | 当前在做的事 / 长期目标（bonsai 本身、GMP 体系…） | 主动跟进「上次说的那个方案怎么样了」 |
| `schedule` | 工作作息（几点开工、周末休） | should_speak 打扰判定可消费（远期，本方案不改决策模块） |

### 不做（隐私红线，写进 key 白名单）

收入、健康、感情状态、精确住址、证件信息。**抽取层用 key 枚举硬编码**——模型/规则只能往白名单 key 里写，白名单外的字段一律丢弃（宁漏勿误，与 extract.rs P0 原则一致）。用户显式说了也不抽，除非后续把 key 加进白名单（需要一次明确的产品裁决）。

## 3. 提取的三条通道

### 3.1 规则层（consolidate 内，确定性，零成本）

`extract.rs` 新增 `PROFILE_PATTERNS`，与 DIRECT_PATTERNS 同构但产出落 profile 而非 facts：

```rust
// (正则前缀, key) — 中文 + 英文，全部要求主语是「我」
("我叫", name)   ("我叫你", 无——这是给阿盆起名，排除)  ("叫我", name)
("我是", occupation)  ("我是做", occupation)  ("我的工作是", occupation)
("我在", city)  + 城市词表校验     // 「我在用 Cursor」不能误进 city → city 必须命中内置城市词表
("我在用", tools)  ("我平时用", tools)  ("我每天用", tools)
("i'm", name/occupation 二义 → 只抽 occupation 语式「i'm a ...」)
```

- 置信度 0.7（与 DIRECT_PATTERNS 对齐）；city 类 0.9（有词表二次校验）。
- `name` 排除词：「阿盆」「你」「它」+ 长度 ≤ 6 字符（中文名合理上限）。
- **抽取入口过 q4 拦截**：onboarding「不准记录」清单命中的 utterance，profile 匹配器与 facts 抽取一样直接跳过（复用 consolidate 现有拦截点，不写第二份）。

### 3.2 模型层（LocalExtractor prompt 扩展，兜住口语化表述）

EXTRACTION_SYSTEM_PROMPT 增加一个抽取目标，输出格式扩展（向后兼容：新字段 `profile` 可缺省）：

```json
{"idx": 0, "fact": "...", "category": "context", ..., "profile": {"key": "occupation", "value": "QA 质量工程师"}}
```

规则：只允许输出 §2 白名单 key；value 一句话、第三人称改第一人称原值；拿不准就输出 null。parse 端白名单外的 key 静默丢弃（宁漏勿误）。模型挂了 → 规则层兜底，管线保证不丢。

### 3.3 用户显式编辑（manual 来源，最高优先级）

工具页画像卡（§6）每行可改可清空。人手改的 `confidence=1.0`、`source='manual'`，**任何后续自动抽取不得覆盖 manual 值**（用户亲口定的身份 > 模型猜的）。

### 3.4 与 facts 的关系（防「重复实现」质疑）

- profile 命中时，facts 侧照旧落 context fact（事件性留痕、总结线/召回可引用）——**一份抽取，两个出口**，不是两套实现。
- prompt 注入只来自 `user_profile` 表；memory_search 行为不变（不查 profile，避免召回结果与注入内容双份重复）。

## 4. 存储：schema v13

```sql
CREATE TABLE IF NOT EXISTS user_profile (
  key             TEXT PRIMARY KEY,           -- name/occupation/tools/city/projects/schedule（白名单枚举）
  value           TEXT NOT NULL,
  source          TEXT NOT NULL,              -- 'rule' | 'model' | 'manual'
  source_event_id INTEGER REFERENCES events(id),
  confidence      REAL NOT NULL DEFAULT 0.7,
  updated_at      TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS user_profile_history (
  id         INTEGER PRIMARY KEY AUTOINCREMENT,
  key        TEXT NOT NULL,
  value      TEXT NOT NULL,
  source     TEXT NOT NULL,
  valid_until TEXT NOT NULL                  -- 被覆盖时间；保留历史供「忘掉前」审计
);
```

- **覆盖策略**：同 key 新值 upsert，旧值进 history（不等价于 invalid_at 语义——身份快照没有「失效时间」，只有「被替换时间」）。
- **冲突仲裁顺序**：manual > (rule | model) 新值。rule 与 model 同日打架取 confidence 高者；都同置信取后到者。
- **`forget_all` 级联清空**两张表——「忘掉一切」包含「忘了你是谁」。
- tools/city 等 P1 key 同表存储，无独立表。

## 5. 注入：system prompt 固定段

`persona/prompt.rs` 的 `PromptContext` 加 `user_profile: &[(key, value)]`，渲染在环境段之后、宪法边界之前：

```
【用户画像】
称呼：许一
职业：QA / 质量管理
常用工具：Cursor、Ollama
```

- 空字段省略；全空 → 整段不渲染（不产生「你知道我什么都不会说」的提示泄漏）。
- 长度硬上限：整段 ≤ 200 字符（画像压缩成短行，不占人格卡预算）。
- 每轮固定注入，**不走 might_recall 门控**——画像不是记忆召回，是身份常量。

## 6. UI：工具页画像卡（展示用户信息）

### 6.1 位置与定位

放**工具页**（`ToolsPane`），插在 `PersonaCard` 之后——「人格卡」是阿盆是谁，「画像卡」是你是谁，两张卡相邻语义对称；BoundaryCard（用户数据管理）已在此页建立交互先例。**记忆页不放**（能力不重复：facts/事件展示归记忆页，身份管理归工具页，一处权威）。

### 6.2 卡片结构 `ProfileCard`

新组件 `frontend/src/components/tools/ProfileCard.tsx`，沿用现有 card 范式（`card / card-head / card-title / card-sub / status-line` class，零散样式写进现有 `styles/tools.css`，不新建 CSS 文件）：

```
┌ 用户画像 ────────────────────────────────┐
│ 它记住了这些 · 点「改」修正 · 记忆可看 = 信任 │
├──────────────────────────────────────┤
│ 称呼      许一            [聊天里记的] 改 清 │
│ 职业      QA / 质量管理    [聊天里记的] 改 清 │
│ 常用工具   Cursor、Ollama  [你写的]    改 清 │
│ （空行 = 不渲染该字段）                     │
│ 还没有任何画像——聊聊天它就慢慢认识你了       │
└──────────────────────────────────────┘
```

- **只展示白名单 key 的中文名**（key→label 映射写死在组件里：name→称呼、occupation→职业、tools→常用工具、city→城市、projects→在做的事、schedule→作息）；白名单 key 之外的数据后端本就不写，前端再兜一层未知 key 丢弃。
- **来源徽标**：`manual`→「你写的」，`rule`/`model`→「聊天里记的」。这是 overview.rs「记忆可看 = 信任」原则的延续——用户能看到哪些是它猜的、哪些是自己定的。
- **每行两个操作**：「改」→ 行内编辑（该行变 input + 保存/取消，不用 `<dialog>`——单行短文本，行内更轻）；「清」→ `profile_delete`（该 key 整条删除，进 history）。
- **空态**：全空显示引导文案，不放 DEMO 假数据（画像是事实数据，假数据会误导——与 BoundaryCard 的 DEMO 不同：边界有既有语义，画像是渐进收集的）。
- 状态全部组件局部 `useState` + `active` prop 触发 load（与 BoundaryCard 同款），**不上 store**——单卡数据无跨页共享（AGENTS.md §状态管理）。

### 6.3 数据链路

| 层 | 内容 |
| --- | --- |
| 后端命令 | `profile_list` / `profile_set` / `profile_delete`（M0 db 层已有 ensure/upsert/get_all/delete，命令薄封装），注册进 lib.rs |
| `lib/commands.ts` | `profileList(): Promise<ProfileEntry[]>` / `profileSet(key, value)` / `profileDelete(key)` |
| `lib/types.ts` | `ProfileEntry { key: string; value: string; source: 'rule' \| 'model' \| 'manual'; confidence: number; updatedAt: string }` |
| 保存语义 | `profile_set` 一律落 `source='manual'`（从 UI 进来的都是用户亲口定的） |

### 6.4 测试

`frontend/tests/ProfileCard.test.tsx`（与 PersonaCard.test.tsx 同构）：

1. 渲染：mock `profileList` 返回两条 → 断言中文名 + 来源徽标；
2. 编辑：点「改」→ input 出现 → 保存 → 断言 `profileSet` 被调 + toast + 重拉；
3. 清除：点「清」→ 断言 `profileDelete` 被调；
4. 错误：list reject → 「用户画像读取失败」status-line；
5. 空态：返回 [] → 引导文案，无 DEMO。

## 7. 分期

| 里程碑 | 内容 | 门禁 |
| --- | --- | --- |
| M0 | schema v13（user_profile + history）+ db 层 ensure/upsert/get_all/delete/forget_all 级联 + `profile_list/set/delete` 命令注册 + 单测 | cargo test 全绿，clippy 零新增 |
| M1 | 抽取接入：RuleExtractor PROFILE_PATTERNS + LocalExtractor prompt/parse 扩展 + consolidate apply（含 q4 拦截、manual 保护、白名单硬校验） | cargo 全绿 + 白名单外 key 丢弃的单测锁死 |
| M2 | prompt.rs 注入段 + dialogue system_prompt 取数 | cargo 全绿 |
| M3 | 前端 ProfileCard（§6）+ commands/types + tests | tsc + vitest 全绿 |
| 验收 | 真机：说「我叫许一，做 QA」→ 重启后问「我是谁」能答；工具页看到来源徽标；手改职业 → 卡片即时更新且旧值进 history；「清」后 prompt 不再注入该字段 | pnpm app 手测 |

## 8. 开放问题（实施前需裁决）

1. **onboarding 要不要加「怎么称呼你」一问？** 建议 P0 不加（5 问已够重，靠对话抽取 + 手动编辑兜底），P1 再评估。
2. **name 的英文/昵称形态**（Jack vs 许一）是否允许多值？建议单值 + history，用户编辑时自己写「许一（Jack）」。
3. **`schedule` 要不要接进 should_speak 打扰判定**——涉及关系线/形态线交叉，本方案明确不做，留到 P2 屏幕感知一起裁决。

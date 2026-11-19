# 方案：边界问答 onboarding（MVP P1 第 3 项）

> **状态**：G1–G4 已实施（2026-11-19，122 单测全过）；紧急联系人按开放问题 2 的建议未做，q4 只做抽取入口拦截不做回溯扫描
> **日期**：2026-11-19
> **依赖**：boundaries 表（schema v3，已落地）、persona 卡宪法边界 b1–b3（已落地）、prompt.rs 用户边界注入（已落地）、extract.rs 对话中边界抽取（已落地）、clock 可注入时钟（已落地）
> **对应调研**：docs/zh/research-report.md §5.3 边界与反谄媚、§7 合规（第 12/13/14 条）、§8.3 P1 第 3 项

## 1. 目标

调研报告对这项的定义只有一句话：**「onboarding 里 5 个问题，专门问『什么话不能说』」**。拆开是两件事：

1. **首次启动的引导流程本身**（现在完全没有：打开窗口直接是三页签，无任何 first-run 检测）；
2. **用 5 个问题把「它对你来说什么不能说」问出来、存进 boundaries、在下一次对话就生效**。

同时调研 §7 的硬性要求必须搭这班车：**「年龄与危机干预必须做进架构，不能后补」——onboarding 从第一天就要设计这三步**（年龄收集、危机干预、未成年人开关）。既然这是本项目第一个 onboarding，框架按合规三步 + 边界问答一起搭，边界问答是核心，合规是骨架。

## 2. 现状盘点（只列和本项相关的）

| 组件 | 现状 | 缺口 |
| --- | --- | --- |
| `memory/db.rs` boundaries 表 | ✅ 有 `add_boundary` / `boundary_exists`（归一化去重）/ `boundaries()` 读 | 无 onboarding 来源标记；无 settings 类 KV 表存「是否完成引导」「年龄段」 |
| persona.json l1.boundaries | ✅ 三条宪法边界（外貌 / 情绪讲道理 / 主动提隐私），带 detect 关键词 + stance | 这是「阿盆的底线」，不随用户变——onboarding 问的是「你的底线」，两套不能混 |
| persona/prompt.rs | ✅ `user_boundaries` 已渲染在宪法之后、表达层之前 | 无需改动，注入链路直接复用 |
| memory/extract.rs | ✅ 对话中说「别再 X」→ category=boundary，跳过置信度门槛直接入表 | **对话中途的边界捕获已工作**，本项只做「开局的批量收集」，两者共用一张表 |
| persona/eval.rs | ✅ 10 个边界探针场景，宪法边界穿透率必须 10/10 | 用户自建边界无探针（free text 没法预先写场景），改为运行期指标 |
| 前端 | ✅ 三页签（对话/表情/工具） | 无引导页、无边界清单管理入口 |
| 合规三步 | ❌ 全无 | 年龄收集、危机干预、未成年人开关都要从零搭 |

关键定位：**宪法边界（b1–b3）是人格的一部分，人人一样；onboarding 边界是用户个体数据，人人不同。** 前者住 persona.json，后者住 boundaries 表，prompt 里分两段渲染——这个分层已经存在，本方案不打破。

## 3. 机制设计

### 3.1 Schema v5：settings + onboarding 审计

```sql
CREATE TABLE IF NOT EXISTS settings (
  key        TEXT PRIMARY KEY,
  value      TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS onboarding_answers (
  id         INTEGER PRIMARY KEY AUTOINCREMENT,
  question   TEXT NOT NULL,              -- 问题标识 q1..q5 / 'age' / 'crisis'
  answer     TEXT NOT NULL,              -- 原始回答或所选选项
  skipped    INTEGER NOT NULL DEFAULT 0, -- 1 = 用户跳过
  ts         TEXT NOT NULL
);
```

- settings 存：`onboarding_done`（0/1）、`age_group`（adult / minor）、`crisis_ack`（看过危机资源卡）。
- onboarding_answers 是审计表：**跳过也记录**（skipped=1），将来改进问题措辞有数据可依。
- `forget_all` 级联清空两张表，onboarding_done 清掉后下次启动重新引导——「忘掉一切」的语义包含「重新认识你」。
- 老用户（已有 events 的库）：`onboarding_done` 缺省视为未完成，**下次启动也会弹引导**，但问题措辞不假设它是新用户；生日锚点等已有数据不重置。

### 3.2 五个问题（核心交付）

每个问题 = 问题文案 + 输入形态 + 落库策略。全部可跳过（年龄除外），跳过不写 boundaries 只写审计。阿盆的语气贯穿：问的是底线，不是问卷调查。

| # | 问题（阿盆式措辞） | 形态 | 落库 |
| --- | --- | --- | --- |
| q1 | 「有什么话题是我永远不该主动提的？比如钱、家人、某段经历。」 | 自由文本（可多条，`、`/`，`/换行分隔） | 每条 → boundaries 一行，content=「不要主动提：X」 |
| q2 | 「有什么话是我永远不该对你说的？比如『你应该冷静』这种。」 | 自由文本（多条） | 每条 → boundaries，content=「不要说：X」 |
| q3 | 「半夜可以打扰你吗？还是加班才算紧急情况？」 | 单选：随便打扰 / 只在工作时间 / 永远别主动 | → preference（复用主动搭话限流已有偏好位）+ settings |
| q4 | 「我会把你记在本地数据库里。有哪类事是不准记的？」 | 自由文本（多条，可跳过） | 每条 → boundaries，content=「不准记录：X」+ 停止入库标记 |
| q5 | 「我记性不好，猜错你别惯着我，直接骂。先说好：什么语气你能接受？」 | 单选：直说就行 / 委婉点 / 别猜我 | → preference（预测反馈与反谄媚措辞基调） |

设计取舍：

- **q1/q2/q4 是真边界**（进 boundaries 表，prompt 注入后立刻生效）；q3/q5 是**偏好**（进 preferences 表，喂给主动搭话限流与预测循环）——不把偏好伪装成边界，表语义不稀释。
- q4 是唯一有删除语义的问题：「不准记录」的条目写入后，**extract.rs 在抽取前比对，命中则丢弃该事件内容只记计数**——这条要在 plan 里写死，因为它是唯一影响数据写入路径的边界。
- **宁缺毋滥原则延续**：自由文本转写只做机械拆分（分隔符切条 + 去空白），**不做语义改写**。用户写「别提我妈」就存「不要主动提：我妈」，不润色、不扩写——转写错一条边界比漏十条严重。

### 3.3 合规骨架（搭车项，范围克制）

1. **年龄收集**（第 12 条）：onboarding 第一屏，二选一（成年 / 未成年）。选未成年 → 显示「未成年人暂不可用」终态页，主窗口不进入，仅可退出。具体年龄不收集（最小化原则）。
2. **危机干预**（第 13/14 条）：不收集紧急联系人（MVP 不做，见开放问题 2），只做两件事：
   - onboarding 中展示一次危机资源卡（北京心理危机研究与干预中心热线等公开资源），确认已读记 `crisis_ack`；
   - 运行期**关键词拦截**：对话输入命中危机词表（「不想活」「轻生」等小词表，宁误勿漏）→ 阿盆停止常规回复，返回固定资源卡文案。规则实现、无模型依赖，与 llm 挂不挂无关。
3. **未成年人开关**：`age_group=minor` 即不可用开关本身；settings 里保留 `minor_unlocked` 键位但不暴露 UI（海外市场将来接家长同意流程用）。

### 3.4 首次启动检测与流程

- Rust `onboarding_status` 命令：返回 `{ done, age_group, answers_summary }`，前端窗口加载时调用。
- 未完成 → 前端覆盖层引导页（360×460 同尺寸，替换三页签视图），完成后 `onboarding_complete` 命令落 `onboarding_done=1` 并 emit `onboarding:done` 事件切回主界面。
- **完成前主功能可用性**：年龄屏是硬门槛（未成年直接终态），其余问题在完成页之前不阻塞——但引导页本身是模态的，即「必须走完流程（含全跳过），才能进主界面」。
- 托盘菜单加「重新做一次引导」项：清 `onboarding_done` 不清答案，下次开窗重走，答案预填可改。

### 3.5 生效链路（G4 验收核心）

```
onboarding 答案
  ├─ q1/q2/q4 → boundaries 表 → prompt.rs user_boundaries 段（已存在，无需改）
  ├─ q4 命中条目 → extract.rs 入库前拦截（新增）
  ├─ q3/q5 → preferences 表 → 主动搭话 / 预测循环读偏好（已存在）
  ├─ 危机词表 → dialogue.rs chat_turn 入口拦截（新增，规则级）
  └─ age_group → settings → 启动 gate（新增）
```

运行期指标（常驻 CI，模式同 predict/morph）：

1. **转写保真率**：每个非跳过答案必须有对应落库行且内容逐字一致（含分隔切条数）——必失败反例：答案含两条但只落一条。
2. **幂等率**：重复 complete / 重复提交同一答案，boundaries 不重复（boundary_exists 兜底）——必失败反例：连点两次完成按钮产生重复行。
3. **拦截正确率**：危机词表命中必拦、无关词不拦（含「不想活了」变体与「活儿干完了」这类误报样本）；q4 边界在抽取路径必拦、非边界内容必放行。

## 4. 里程碑

| 里程碑 | 内容 | 规模 | 依赖 |
| --- | --- | --- | --- |
| **G1** | schema v5（settings + onboarding_answers）+ forget 级联 + `onboarding_status`/`onboarding_complete` 命令 + 老用户兼容 | 0.5 周 | 无 |
| **G2** | 五问引擎：问题定义、答案→boundaries/preferences 转写（机械切分、逐字保真、幂等）+ q4 抽取拦截 | 0.5 周 | G1 |
| **G3** | 前端引导页（年龄屏 → 合规卡 → 五问 → 完成页）+ 工具页边界清单（查看/删除自己写的边界）+ 托盘重引导入口 | 1 周 | G2 |
| **G4** | 危机关键词拦截 + 三指标 eval + README 双语进度表与 plan 状态更新 | 0.5 周 | G3 |

全程不依赖真模型；规则后端下边界 prompt 注入同样可测。

## 5. 开放问题（待拍板，均给了建议默认值）

1. **年龄收集粒度**：建议二分（成年/未成年）而非具体年龄——最小化收集，第 12 条只要求「收集年龄」确认未成年身份即满足；具体生日没有下游用途。
2. **紧急联系人**：建议 MVP 只做危机资源卡 + 关键词拦截，联系人收集延后到有真实用户运营时——现在收集了也没有调用链路，属于为合规而合规的数据负债。
3. **onboarding 可跳过性**：建议除年龄屏外全部可跳过（含危机确认卡可跳过阅读但记录 skipped）——强迫读完五问会劝退，而跳过行为本身就是数据。
4. **q4「不准记录」的执行深度**：建议先做抽取路径拦截（extract.rs 前置比对），**不做**已存数据的回溯扫描——回溯删除涉及事实/事件/向量三层联动，单独一个方案更稳妥。

## 6. 合规对照

| 《拟人化办法》条款 | 本方案的落点 |
| --- | --- |
| 第 12 条（收集年龄） | G1 settings.age_group + G3 年龄屏 |
| 第 13/14 条（危机干预） | G3 资源卡 + G4 关键词拦截 |
| 未成年人保护 | age_group=minor 终态页（默认不可用） |
| 第 8 条(5) 禁止过度迎合 | 边界 prompt 注入 + 反谄媚基调问题 q5（已有 disagreement_policy 兜底） |

## 7. 实施备注

- 实施前需确认：`minor_unlocked` 等预留键位不暴露 UI；危机词表初版收敛在 ≤10 词，人工过一遍误报样本。
- 本项完成后 MVP P1 三项全部闭环，README 进度表「边界问答 ⬜」改 🔨；下一阶段进入 P2 或打包发布实测（sidecar 进 bundle，见 morph 方案遗留项）。

## 8. 实施记录（2026-11-19，G1–G4 全部落地）

- **问题文案单源**：`QUESTIONS` 随 `onboarding_status` 下发给前端，JS 不再保存第二份文案（预览模式才用 fallback 副本）。
- **危机拦截位置**：`dialogue::chat_turn_inner` 入口、模型调用之前；`DialogueOutcome.crisis` 标记让前端显示「危机资源 · 常规回复已拦截」，不会误报「本地模型未就绪」。双向仍记录事件（合规审计）。
- **q4 拦截位置**：`consolidate::run_in_tx` 的抽取入口，`Report.blocked` 计数；原文事件保留（它是 ground truth，forget 路径依赖它），只是永不成为事实/偏好/边界。
- **未成年门禁双层**：前端终态页（不可进入主界面）+ 命令层 `ensure_not_minor`（`dialogue_chat` / `dialogue_chat_stream` / `dialogue_proactive_now` 全部拒绝）。
- **q3/q5 表语义**：写 preferences（q3→habit，q5→context），不伪装成 boundaries；重复提交靠 `promote_fact` 的内容去重保证幂等。
- **边界管理**：工具页「边界清单」列出全部边界（含对话中途抽取的），可逐条删除——合规第 16 条对每条边界同样适用。
- **托盘重引导**：只清 `onboarding_done`，答案保留；`window:shown` 时前端重查状态并弹回引导。

# bonsai（盆栽）

一个陪你一起长大的桌面 AI 伙伴。本地优先的记忆、有自己脾气的人格、彻底开源。你的数据永远不离开你的电脑。

**English docs** → [README.md](./README.md)

## bonsai 是什么？

> 一个住在你电脑里、有点脾气、会慢慢学会你的搭子。它先帮你干活，然后才谈得上陪你。

bonsai 的立项判断是：介于「AI 陪伴」与「AI 助手」之间的中间地带，是唯一没有被市场证伪的缝隙——纯情感陪伴的单位经济已经破产，而通用助手没有「关系连续性」。bonsai 用三个刻意选择占据这个缝隙：

- **先干活** —— 先做一件真实、有用的活（合规友好，经济上成立）
- **有点脾气** —— 反谄媚靠产品机制强制执行，不靠模型自觉
- **慢慢学会** —— 成长绑定在「理解准确度」上，而不是「互动时长」上

明确不做的事：不做恋爱/虚拟伴侣、不做角色扮演社区、不做角色商店、不做无限对话时长。

## 架构（7 层）

```text
L6 呈现层   一张脸（12–20 帧状态机动画）+ 托盘图标
L5 生成层   人格卡注入 → 本地 LLM 对话；反谄媚过滤器
            降级：端侧小模型接管（能力降级，人格不降级）
L4 决策层   「要不要开口」可打断性模型 + 成长事件判定
L3 人格层   persona.json（traits/values/quirks/boundaries）+ 漂移检测器
L2 记忆层   SQLite：事件 / 事实 / 偏好 / 边界 + 向量索引
            夜间整理：压缩 → 抽取 → 消解矛盾 → 删除
L1 感知层   前台 App、idle、键鼠节奏 —— 原文不出机器
L0 壳层     Tauri 2（Rust core），菜单栏常驻，开机自启
```

三个关键取舍：

1. **人格层独立于模型** —— 人格卡是纯文本 + 状态机，换模型只换表达，不换人。
2. **对话与感知原文不出机器** —— 默认模型是本地 LLM（Ollama 兼容 API）；云端 API 仅在用户显式开启后才可用。
3. **决策层独立于生成层** —— 「要不要开口」≠「说什么」，阈值、A/B、打断成本单独调。

## 平台与技术栈

- **macOS 先发**，Windows 第二批
- **外壳**：Tauri 2.x（Rust core）
- **记忆**：本地 SQLite，hybrid 检索（向量 + 关键词 + 时间衰减）
- **模型**：本地 LLM 优先（Ollama 兼容 API，跑在本机）——对话/抽取/整理全部离线可用；云端 API 仅作为用户显式开启的可选质量增强

## MVP 路线图（8–10 周）

| 档位 | 范围 |
| --- | --- |
| P0 | macOS 菜单栏常驻、情绪脸（12 帧状态机）、三层记忆 + 夜间整理、人格卡 v1 + 一致性测试集、每日 1 次主动搭话（严格限流）、一件真活（把当天剪贴板/下载整理成一页日报） |
| P1 | 预测-验证循环、形态线成长（7 天可见变化）、边界 onboarding 问答 |
| P2 | 语音、屏幕感知、工具调用、关系线成长、跨设备 |

## 运行（骨架阶段）

前置要求：Rust 1.77+、Node.js 22+、[pnpm](https://pnpm.io) 10+。前端由 Vite 管理（暂未引入框架）；`@tauri-apps/cli` 是开发依赖，无需全局安装 tauri-cli。

**本地 LLM（声明式依赖，安装进项目内）**——Ollama 二进制以 Tauri sidecar 形式随应用打包，由应用自己托管：启动时拉起（端口 11435，绝不碰系统级 Ollama 的 11434）、退出时回收；模型存放开发期为项目内 `.llm/models`、打包后为应用数据目录。一次性手动步骤只有两步：

```sh
pnpm llm:sidecar          # 把 Ollama 运行时（CLI + llama-server 推理器）取回 src-tauri/binaries/
pnpm llm:pull             # 拉取 qwen3.5:2b 到 .llm/models（默认模型）
```

没做这两步也不影响任何功能：构建期会自动生成占位 sidecar，模型腿自动降级为确定性的规则兜底（日志里只有一行警告）。可用环境变量覆盖：`OLLAMA_HOST`（服务地址）、`BONSAI_LLM_MODEL`（模型 tag）。

```sh
pnpm install      # 安装前端依赖（vite + @tauri-apps/api + @tauri-apps/cli）
pnpm check        # 验证 Rust 核心（cargo check）
pnpm test         # Rust 单元测试（记忆、决策、生成、人格）
pnpm app          # 运行桌面应用（tauri dev；自动启动 vite :1420）
pnpm app:build    # 生产构建
```

Rust 核心已实现：L1 感知层（逐项授权、默认关闭）、L2 记忆层（SQLite，本地优先）、L3 人格卡、L4 主动开口决策模型、L5 反谄媚过滤器 + 对话管线、本地 LLM 接入（Ollama 兼容）与日报工具——均带单元测试。

## 文档

- 中文
  - [调研报告](./docs/zh/research-report.md) — 立项依据：市场、可行性、记忆、人格、养成、合规、MVP、风险
  - [方案：三层记忆 + 夜间整理](./docs/zh/plan-memory.md) — MVP P0 第 3 项的实施计划（M1–M4 已实施）
  - [方案：人格卡 v1 + 一致性测试集](./docs/zh/plan-persona.md) — MVP P0 第 4 项的实施计划（PM1–PM3 已实施）
  - [方案：预测-验证循环](./docs/zh/plan-predict.md) — MVP P1 第 1 项的实施计划（G1–G4 已实施）
  - [方案：形态线成长](./docs/zh/plan-morph.md) — MVP P1 第 2 项的实施计划（G1–G4 已实施）
  - [方案：关系线成长](./docs/zh/plan-relationship.md) — MVP P2 第 1 项的实施计划（Q1–Q4 已实施）
  - [方案：日历 / 邮件工具调用](./docs/zh/plan-calendar.md) — MVP P2 第 2 项的实施计划（T0–T4 已实施，邮件 T5 未做）
  - [方案：MCP 调用 + Skill 支持](./docs/zh/plan-agent-mcp-skill.md) — 通用 Agent 能力（S1–S6 已实施，spec 见 [docs/zh/specs/agent](./docs/zh/specs/agent/README.md)）
  - [Runbook：L3 真机验收短清单](./docs/zh/runbook-l3-acceptance/v1/index.md) — 发版前人工勾选（测试绿 ≠ 产品效果）
- English
  - [Research report](./docs/en/research-report.md) — founding research: market, feasibility, memory, personality, growth, compliance, MVP, risks

## 当前状态

🧱 **可用阶段** —— 工具链：pnpm + Next.js（App Router，`frontend/`）+ Tauri 2（`pnpm app` 运行）。P0/P1 六项 + P2 关系线/日历工具 + MCP/Skill 管线代码已落地；界面按侧栏分「对话 / 表情 / 记忆 / 工具 / 设置」五页。

MVP P0 进度（调研报告 §8.3）：

| P0 项 | 状态 |
| --- | --- |
| macOS 菜单栏常驻 + 托盘 + 开机自启 | 🔨 已实现，未跑 `pnpm app` 实测 |
| 情绪脸：12 帧状态机 | 🔨 Rust 持有状态机（按时间推导基态 + 瞬态表情 + `face:state` 事件广播），前端渲染 6 状态 × 2 帧；待运行实测 |
| 三层记忆 + 夜间整理 | 🔨 三层落地（事件→事实→偏好/边界）：schema v2 迁移、规则抽取（`FactExtractor` trait，模型层走同一接口，本地 LLM 优先）、consolidate 五阶段管线（抽取→合并→矛盾消解→升格/删除）、03:00 调度 + catchup 补跑 + 双重幂等、时间衰减检索；三指标评估（命中率>85% / 误记率<1% / 衰减正确率 100%）为常驻 CI 测试（见 docs/zh/plan-memory.md） |
| 人格卡 v1 + 一致性测试集 | 🔨 卡 v1.1（L1/L2 显式分层 + 边界结构化 + 反对预算机器可读 + v1 向后兼容 + 强校验）、prompt 确定性渲染器（L3 引用 face 状态）、20 场景 × 5 轮场景集、评测 harness（Mock 后端 CI 全量达标：穿透 10/10 守住、反对率 8/10、漂移 0）；真模型跑分挂云端管线（见 docs/zh/plan-persona.md） |
| 每日 1 次主动搭话 + 限流 + 可一键关闭 | 🔨 对话管线拼装完成（persona prompt → 本地 LLM → 反谄媚过滤器 → 记忆层记录，`dialogue_chat` 命令）；主动搭话线程 60s 轮询 L4（每日 1 次配额 + idle>5min + 忙不打断），模型挂时静默不装活；一键关闭走感知层总开关 + 配额；待运行实测 |
| 一件真活（剪贴板/下载整理成日报） | 🔨 日报完成：按本地日聚合感知事件 + 记忆增量渲染为一页 markdown（`daily_report` 命令），本地 LLM 一句话总结可选、失败降级规则拼接，报告落盘 app_data/reports/ |

关键路径上的阻塞项：`pnpm app` 实测验证、本地模型（Ollama）真机验证。

MVP P1 进度：

| P1 项 | 状态 |
| --- | --- |
| 预测-验证循环 | 🔨 已实现：每天最多 1 次证据可溯源的猜测（habit 直推 / 14 天节律外推 / 连续性猜），三档反馈（对/错/别烦我）回写偏好置信度，「别烦我」退避 3 天；与主动搭话互斥、busy 不打断；指标门槛（命中率 ≥50%、退避率 <10%）为常驻 CI 测试（见 docs/zh/plan-predict.md） |
| 形态线成长（7 天可见变化） | 🔨 已实现：四阶段 7 天节奏（破土 day0–1 / 抽枝 day2 / 成形 day4 / 成景 day6），靠时间推进、无签到无数值；schema v4 growth 表（出生锚点从最早事件回填、阶段单调不减、daily tick 幂等）；日微变化（叶色渐深 + 按日确定性加小叶 + 阶段自述）；休眠而非死亡（7 天无互动 → 落叶待机，回来即恢复）；升阶播报只在开窗断点播一次（`notified_at` 幂等，不占打扰额度）；S3 解锁「得意」表情与 poke 文案分级；三指标（升阶准时率 / 播报幂等率 / 休眠正确率）为常驻 CI 测试（见 docs/zh/plan-morph.md） |
| 边界 onboarding 问答 | 🔨 已实现：首次启动模态引导（年龄屏硬门槛 → 危机资源卡 → 五问 → 完成页，除年龄外全可跳过）；schema v5 settings + onboarding_answers 审计表（跳过也记录）；q1/q2/q4 逐字保真切条写入 boundaries（prompt 已自动注入生效，重复提交幂等），q3/q5 写 preferences 而非边界；exact「不准记录」条目在记忆抽取入口拦截（原文保留、绝不成为事实）；危机词表规则级拦截（模型挂也生效）+ 未成年人在对话命令层二次门禁；托盘「重新做一次引导」保留答案重走流程；工具页边界清单可查看/删除；三指标（转写保真率 / 幂等率 / 拦截正确率）为常驻 CI 测试（见 docs/zh/plan-onboarding.md） |

MVP P2 进度：

| P2 项 | 状态 |
| --- | --- |
| 关系线成长（8–30 天，靠理解准确度） | 🔨 已实现：三阶段（拘谨 → 熟络 day≥9 且证据≥12 → 会顶嘴 day≥22 且证据≥30，双条件、天数只给窗口不给进度）；证据每日从 predictions + events 重算（在场/猜中/顶嘴没被拒 各 +1 上限，猜错 −1 不累积），分数永不出进程、不上 UI；「别烦我」不扣分只暂停推进（退避率 >10%）；7 天无互动 → 生疏（语气收回，阶段不退）；prompt 注入四档关系模式，R2 把缓冲句禁令写成硬约束；反谄媚命中首次落库为 disagreement / disagreement_hedged 事件；R2 后 7 天内「第一次没顺着你」彩蛋（只播一次）；工具页一行 + 重置（只清关系线）；四指标为常驻 CI 测试（见 docs/zh/plan-relationship.md） |
| 日历 / 邮件工具调用 | 🔨 已实现（日历只读；邮件未做）：schema v7 `calendar_cache`（TTL 5 分钟、标题只活在这里、`reminded_at` 幂等）+ `tool_calls` 审计（只有动作与计数，无内容）；EventKit 在本进程内调用（TCC 归属「责任进程」，helper 会分裂成两个身份），主线程发授权请求 + 60s 超时，**只认 `fullAccess`**（`writeOnly` 不等于能读）；拒绝/未授权一律返回 `calendar_denied` 等错误码，绝不拿空列表冒充「今天没会」；规则意图路由（时间词 + 日程名词双命中，20 正 / 15 负样本 CI）接在危机拦截之后、模型调用之前，模板作答不依赖模型；会前提醒是 L4 的第二条 lane（−10±2 分钟窗口、日 ≤3、间隔 ≥10 分钟、22–08 静默、会中不提、全天/已拒/已取消不提、点「今天别再提醒」当天全静）；`in_meeting` 接进 `should_speak`（参会人 ≥2 才算会，单人专注块不闭嘴）；日报加「明天」节（无日历 / 未授权 / 有安排三态区分）；工具页日历卡 +「它今天读了 N 次」；邮件（AppleScript）拆为 T5 未做（见 docs/zh/plan-calendar.md §3.9/§11） |
| MCP 工具 / Skill 包 / 调用审计 | 🔨 已实现（P3-4 提前落地，见 docs/zh/plan-agent-mcp-skill.md）：schema v8 `mcp_servers`；MCP 双传输（stdio 子进程 + Streamable HTTP 远程），rmcp 官方 SDK 收在 `mcp.rs` 内、对外同步门面，connect 10s / list+call 60s 硬超时，不可达一律 `mcp_unreachable` 不冒充空结果，退出 `shutdown_all` 防孤儿进程；新 server 一律默认停用（RK-04），headers/env 明文只存本机 SQLite、所有读路径脱敏；**三级渐进注入**：system prompt 只带 name（≤20）→ 模型点候选（白名单 ≤5）→ 注入完整描述/输入 schema → 调用 → 结果摘要作答，**无候选时零额外轮次**（与旧行为无可感差异）；Skill 为纯提示注入（正文 ≤8KB、引用 ≤4KB×5、注入黑名单拒载），同时最多注入 2 个全文；审批钩子 `ApprovalGate`（默认 `auto` 放行；切 `ask` 弹确认卡，30 秒不回应视为拒绝）；`tool_calls` 审计含 params 脱敏摘要 / result_status / prompt_trace 引用 / remote 标记，工具页可查、对话气泡带 🔧 标注；环境段（时间/星期/时区/语言/平台）由系统注入且声明不可变；281 单测 + 70 前端测试覆盖 |
| 语音（偶发语音） | ⬜ 未开始 |
| 屏幕感知 | ⬜ 未开始 |
| 跨设备 / 数据可携带 | ⬜ 未开始 |

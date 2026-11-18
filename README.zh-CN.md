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
pnpm llm:sidecar          # 把 Ollama 二进制取回 src-tauri/binaries/
pnpm llm:pull             # 拉取 qwen3:4b 到 .llm/models（默认模型）
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
- English
  - [Research report](./docs/en/research-report.md) — founding research: market, feasibility, memory, personality, growth, compliance, MVP, risks

## 当前状态

🧱 **骨架阶段** —— 工具链：pnpm + Vite + Tauri 2（`pnpm app` 运行）。P0 六项功能代码已全部落地，界面含「对话 / 表情 / 工具」三页（日报、整理记忆、感知开关、手动主动搭话），待 `pnpm app` 实测。

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

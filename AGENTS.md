# AGENTS.md — bonsai 工程约束规范

> 本文件是项目约定文档，所有 AI 协作与人工开发必须遵守。
> **MUST**（硬约束）违反不允许提交；**SHOULD** 为强烈建议，可经豁免流程偏离。
> 本文件自身长度 **MUST ≤ 700 行**；细则与历史裁决放 `docs/`，此处只留可执行条款。

## 适用范围

- `frontend/`：Next.js 14 + React 18 + TypeScript 5.7 + Tailwind CSS 3.4 + zustand 5（含样式/组件条款）。
- `src-tauri/`：Rust / cargo clippy / 模块分层；样式类条款不适用。
- 下列章节为**全项目硬约束**（前后端均适用）：§工程硬约束、§模型调用、§能力与架构、§质量门与变更、§例外流程。

---

## 工程硬约束（全项目 MUST，对应业界适配 1–10）

### 1. 信任边界（Tauri）

- WebView / 前端代码**不可信**；Rust core **可信且权限最大**。
- 所有 IPC / `invoke` 入参、路径、URL、scope **MUST** 在 Rust command 内校验并 enforce；前端校验只做 UX，不算安全边界。
- 密钥、token、DB 连接、特权操作、全局权威状态 **MUST** 留在 Rust；**MUST NOT** 下发到前端或日志明文。
- capabilities / asset / fs / http scope **MUST** 最小权限；新窗口或新 command 默认拒绝，显式授权。
- 新增或放宽 capability / CSP **MUST** 在 PR 说明攻击面变化。

### 2. Local-first / 隐私

- 默认对话、记忆、感知原文 **MUST NOT** 离机；云端 API **MUST** 用户显式 opt-in 后才可用。
- 感知层（剪贴板、前台应用、输入节奏等）原始文本 **MUST** 仅本地处理；导出/同步另走独立、可关闭路径。
- 新增「默认上传 / 隐式遥测 / 静默云调用」**MUST NOT** 合入。

### 3. 分层不越权（L0–L6）

- 人格（persona）独立于模型；换模型只换表达，不改人设权威源。
- 「是否开口」（decision）与「说什么」（generation）**MUST** 分离，禁止为便利揉成一路。
- **MUST NOT** 跨层复制权威逻辑（例如在前端重写记忆检索、在 command 层绕开 tool 再实现一份日历）。
- 新代码落入与现有模块同层同目录；不确定时先对齐邻近模块再扩展。

### 4. 边界运行时校验

- 跨信任边界的数据（invoke 参数、tool/MCP 入参与返回、云端响应、env）**MUST** 先校验再使用；TypeScript 类型 ≠ 运行时安全。
- 环境变量经集中读取/校验入口使用，避免业务代码散落 `process.env` / 未校验 `std::env`。
- 失败路径返回可区分错误，**MUST NOT** 用「假装成功」或默认值掩盖校验失败（产品明确降级除外，且需日志）。

### 5. 质量门可机检

任务「完成」**MUST** 满足（按改动范围执行适用项）：

| 范围 | 命令 |
|------|------|
| Rust | 根目录 `pnpm check`；相关 `pnpm test` |
| 前端 | `cd frontend && npx tsc --noEmit`；`npm run test`；改组件时 `npm run lint` |

- **MUST NOT** 删除、跳过或弱化测试，或吞错 / 整文件关 lint/type 来「变绿」。
- 新增组件 / hook / 公共 lib / 关键 Rust 路径 **MUST** 附最小测试（`frontend/tests/` 或 crate 单测）。
- 无法自动化验证时，在回复/PR 写明未验证项与原因。

### 6. 变更最小化

- 每行改动可追溯到当前任务；无关问题记入说明，不塞进同一 diff。
- **MUST NOT** 擅自新增依赖、抽象层、配置项或「顺手重构」。
- 解决当下已知问题，不为假想未来过度设计。

### 7. 事实来源单一

- 能力实现、产品裁决、工程约束各保留**一份权威**；禁止平行副本。
- 文档按 `docs/<domain>/<version>/` 组织，版本含 `index.md`；更新建新版本目录，不覆盖历史（纠错除外）。
- 跨文档用相对路径引用；AI 默认读该领域最新版 `index.md`。
- 详细设计/runbook 放 `docs/`，本文件只保留可执行约束。

### 8. 规则分层（MUST vs SHOULD）

- **MUST**：本文件标注硬约束 / 「不允许提交」的条款；违规禁合入。
- **SHOULD**：强烈建议；偏离须在 PR/commit 写原因与替代方案。
- 豁免见 §例外流程；**MUST NOT** 静默违反。

### 9. 安全与供应链

- **MUST NOT** 提交密钥、凭证、`.env` 实值、私钥；示例用占位符。
- 新增或升级依赖评估必要性与许可证；高风险变更跑 `cargo audit` / `npm audit`（或等价）并处理高危。
- CSP、isolation、外部 origin 白名单宜收紧；`csp: null` 或过宽 scope 的变更 **MUST** 附收口计划或书面风险接受。

### 10. 可访问与可观测（前端 SHOULD，关键路径 MUST）

- 关键交互（发送对话、开关云端、授权 tool、设置项）**SHOULD** 键盘可达、有可见焦点与明确失败反馈。
- 关键失败 **MUST** 可诊断（结构化日志或用户可见错误），**MUST NOT** silent fail。
- 用户可见文案与错误码保持稳定，便于排障与测试断言。

### 11. 耗时任务异步、不阻塞交互（全项目 MUST）

- 预计耗时较长的工作（LLM 推理、模型拉取、记忆整理、MCP/tool 调用、大文件/网络 IO、批量 DB、定时巩固等）**MUST NOT** 在阻塞 UI 的路径上同步跑完。
- **Rust**：长任务放 `async` / 后台任务（`spawn`、专用 worker、scheduler）；command 尽快返回或流式/事件推进度；**MUST NOT** 在主线程或持锁期间做重计算 / 阻塞 IO。
- **前端**：用 async/`invoke` + loading / 进度 / 可取消状态；**MUST NOT** 在事件处理器或渲染路径里同步空等长调用导致页面卡死。
- 进行中 **SHOULD** 保留可操作面（取消、关闭面板、继续浏览）；失败与完成经事件或状态回传，**MUST NOT** silent hang。
- 违反形态示例（均不允许提交）：在 `invoke` 同步路径里拉完整模型、在持 DB 锁时跑 LLM、前端 `await` 长任务期间无反馈且锁死交互。

---

## 前端核心约束（MUST，作用于 `frontend/`）

1. **重复代码上限**：同一逻辑/JSX 块出现 **2 次以上且重复行数 > 10 行**，必须抽到 `src/lib/`、`src/hooks/` 或 `src/components/`。相似 ≠ 相同；差异点参数化。
2. **跨页面复用**：被 ≥2 个 `src/app/` 路由引用的组件必须上移 `src/components/`；页面私有组件就近 feature 目录，一旦跨页立即上移。
3. **优先复用**：新写前先查 `src/components/`、`src/hooks/`、`src/lib/`；有则复用或扩展（可选 props 向后兼容）。
4. **状态管理**：跨组件/跨页用 **zustand**（`persist` 等按需）；局部用 `useState`。store 名 `use{Name}Store.ts`，按领域拆分；派生数据组件内计算；用 selector 按字段订阅。
5. **样式走 Tailwind**：禁止无必要 inline `style`（动态值除外并注释原因）；禁止新建零散 CSS；全局样式进 `globals.css` / theme token，不硬编码颜色间距。

---

## 模型调用约束（全项目 MUST，2026-11-22）

1. 工具与 skill **只在**云端支持 tool calling 的模型上启用（当前 DeepSeek function calling / `cloud_tool_loop`）：内置工具、MCP、skill 注入均随云端管线挂载；本地管线一律不出现。
2. 本地模型（Ollama sidecar）**只做纯聊天、零工具调用**：不挂 tool provider、不发 `tools[]`；**日历 / 邮件无对话规则腿、本地不代答**（仅云端 + 对应开关开时经 function calling 执行）；需工具时诚实告知开启云端模式，绝不伪装执行。本地仅保留 `memory_search` / `perception_search` 的代码层强制召回腿（非 `tools[]`）。
3. **禁止**：本地 chat 接 `tools[]`、本地路径新增工具执行分支、对话前置日历/邮件规则路由、skill 注入本地 system prompt、为本地调工具复活 JSON 协议 hack（三级渐进 JSON 管线已退役）。
4. 「本地零工具调用」约束的是 `tools[]` / function calling（对话管线挂工具）。后台管线（夜间抽取 / 巩固 / 总结）在本地模型上使用**原生结构化输出**（Ollama `format` / 云端 `response_format`）不构成工具调用，不触发本约束——工具的执行者始终是引擎，结构化输出只是让模型把话说成机器能编译的形状（plan-structured-routing，2026-11-24）。

---

## 能力与架构约束（全项目 MUST，2026-11-22）

1. **能力一份权威实现**，载体为内置工具或 MCP；规则路由、intent、前端、skill 只**消费**工具输出，不得平行实现。  
   禁止：Rust 与前端各写一份、既是 builtin 又是独立 skill 包、command 层绕开工具复制查询。  
   范式：总结线由引擎代调 `memory_search`，模型只成文。
2. **AI 优先**：新能力默认交付面是 AI（工具/对话）；UI 是管理与展示面；「给 AI」与「给人点」冲突时默认前者。

---

## 通用规范（SHOULD，业界惯例 + 本项目）

### 组件

- 只写函数组件；一文件一主组件；文件名与组件名一致（PascalCase.tsx）。
- 文件 ≳150 行或 JSX ≳80 行拆分子组件；业务逻辑 ≳50 行抽 `use{Name}` → `src/hooks/`。
- Props：`{ComponentName}Props` 并导出；签名内解构；`children: React.ReactNode`。
- 变体用判别式 props（`variant: 'primary' | 'ghost'`），避免布尔堆叠。
- `clsx` 拼条件 class；Tailwind class 保持静态字符串。

### TypeScript

- `strict`；禁止 `any`；props/回调/state 显式类型。
- 共享类型进 `src/types/` 或就近领域文件。

### 状态与数据

- 能用最近共同祖先解决就不上 store；能派生就不复制 state。
- Tauri `invoke` **MUST** 经 `src/lib/` 封装，叶子 UI 不散落裸调用。

### Tailwind class 顺序

布局 → 尺寸 → 间距 → 文字 → 视觉 → 交互（hover/focus）。

---

## 例外流程

无法满足 MUST 时（性能热路径、第三方限制、过渡期 CSP 等）：在 PR / commit message 说明原因、替代方案与收回条件，经确认后豁免；**不允许静默违反**。

# plan-ui-panel.md — 设置面板：模型配置 / 人格展示 / 记忆总览 / 调试日志

> 2026-11-19 立项。目标：把散落在环境变量和代码里的可配置项搬进界面，并把系统的「内里」做成可查看而非黑盒。
> 本文对应需求六条：① 自定义配置切换模型 ② 自定义云端模型 ③ 展示人格系统 ④ 展示记忆存储 ⑤ 查看 debug 日志 ⑥ 授权读取日历（已实现，验收即可）。
> **交互设计稿**：`design/ui-mockup.html`（单文件、可操作——5 tab 导航、provider 切换、模型卡片选择、下载进度模拟、云端 opt-in 模态、忘记全部两段式确认）。模型配置交互参考 meetily（同机 `~/Desktop/git/meetily`，见 §5.7）。
> **详细 spec（实施用）**：`docs/zh/specs/ui-panel/`（README.md 索引 + M0a/M0b/M1/M2/M2.1/M3/M4/M5 各一份，含接口签名、组件拆分、测试清单、验收标准；本 plan 保留为总纲，实施细节以 spec 为准）。

## 1. 现状盘点

| 需求 | 现状 | 缺口 |
|---|---|---|
| 切换模型 | `BONSAI_LLM_MODEL` 环境变量，`OllamaClient::from_env()` 在 4 处调用点各自构造（dialogue / extract / persona eval / report），改了也不热生效 | 无 UI、无持久化、无运行时切换 |
| 云端模型 | llm.rs 明确注释「deliberately NOT implemented」，完全未实现 | 需要新传输层 + 密钥存储 + 明示授权 |
| 人格展示 | `persona::get_persona` 命令已存在，前端未消费 | 缺展示页；缺「当前生效状态」（语气档/表情）的合成视图 |
| 记忆展示 | 15 张表全部在库，仅边界清单（boundary_list）和关系线一行有 UI；events/facts/preferences/predictions 均不可见 | 缺只读总览命令 + 页面 |
| debug 日志 | `log` crate 打点遍布，但 main.rs **没有初始化任何 logger，日志实际无处可去**；sidecar stderr 已落 `llm-sidecar.log` | 需先接文件 logger，再做 tail 命令 + 页面 |
| 日历授权 | calendar_status / calendar_request_access / 「去授权」按钮齐备（P2-2） | 无缺口，M5 只做验收与文案打磨 |

前端现状：单文件 `ui/main.js`（1386 行）+ `index.html`，现有 tab：对话 / 表情 / 工具（顶部胶囊横排）。本次先做 **M0a 迁移到 Next.js（§2.0，行为不变）**，再在新架构上做 M0b 布局壳重构（侧栏），新内容以组件形式挂进新壳。

## 2. 总体设计

### 2.0 前端架构迁移：vanilla JS → Next.js（先于一切 UI 工作）

> 用户裁决（2026-11-19）：参照 meetily（同机 `~/Desktop/git/meetily/frontend`）把前端从 `ui/`（vite + 单文件 main.js 1386 行）迁移到 Next.js。迁移本身**不改任何行为**，先 1:1 平移，再在新架构上做 M0b 侧栏布局与后续功能。这也与用户长期规范对齐：React + TypeScript + TailwindCSS、文件 ≤500 行、公共组件必须有测试。

**技术栈（对齐 meetily 实测组合，不追新）**：

| 项 | 选型 | 说明 |
|---|---|---|
| 框架 | Next.js 14（App Router）+ React 18 + TypeScript | meetily 同款；`reactStrictMode: false`（流式气泡双触发） |
| 样式 | Tailwind CSS 3 | 现有 `:root` 变量整体搬进 globals.css，tailwind.config 以 CSS 变量扩展色板（视觉零变化） |
| 组件底座 | shadcn/ui（Radix）按需引入 | Switch/Dialog/Select/ScrollArea 等替换手写 toggle/dialog |
| 状态 | zustand，仅两个 store：`uiStore`（toast）、`llmStore`（模型配置/下载进度快照） | 遵守「非必要不加」；active tab 由路由承担，不进 store |
| 测试 | vitest + @testing-library/react | `pnpm test`；公共组件/函数强制带测（用户规则 3） |
| Lint | eslint（next/core-web-vitals）+ prettier | 用户规则 4 |

**Tauri 接线（照搬 meetily 已踩平的路）**：

```jsonc
// tauri.conf.json
"build": {
  "beforeDevCommand": "pnpm --filter frontend dev",
  "beforeBuildCommand": "pnpm --filter frontend build",
  "devUrl": "http://localhost:3000",
  "frontendDist": "../frontend/out"
}
```

- `next.config.js`：`output: 'export'`（静态导出，Tauri WebView 直接加载 `out/`，无 node 服务）；`images.unoptimized`；webpack 照搬 meetily 两处调优——`chunkLoadTimeout: 600000`（dev 按需编译超过默认 120s 会报 ChunkLoadError）+ dev `watchOptions.ignored` 排除 `node_modules/.next/target/out`（macOS soft maxfiles 256 下 EMFILE 导致路由 404）。
- **目录结构**（对齐 meetily 分层）：

```
frontend/
├── src/
│   ├── app/                    # App Router：每个 tab 一页
│   │   ├── layout.tsx          # 侧栏布局壳（M0b 在此落地）+ ⌘1-5 快捷键
│   │   ├── page.tsx            # → 对话（默认路由）
│   │   ├── face/page.tsx       # 表情
│   │   ├── tools/page.tsx      # 工具
│   │   ├── memory/page.tsx     # 记忆
│   │   └── settings/page.tsx   # 设置
│   ├── components/             # 按域分组：chat/ face/ tools/ memory/ settings/ layout/
│   ├── lib/                    # tauriInvoke.ts（15s 超时 fail-fast，meetily 模式）
│   │                           # tauriListen.ts（listen 封装 + safeUnlisten）
│   │                           # stores/（uiStore、llmStore）
│   ├── hooks/                  # usePersona / useMemoryOverview / useModelDownload …
│   └── types/                  # 命令返回类型（与 Rust serde 对齐，单一来源）
├── next.config.js / tailwind.config.ts / package.json
```

**迁移铁律**：

1. **行为 1:1，视觉 1:1**——main.js 1386 行按域拆成 ≤500 行的组件/文件（用户规则 1），事件订阅（face:state / growth:stage / llm:ready / llm:pull:progress / proactive:message…）集中进 `lib/tauriListen.ts` 消费；流式气泡的 `Channel` 用 ref 管理，避免 StrictMode/重渲染双订阅。
2. 所有触碰 `invoke`/`window` 的组件标 `'use client'`；页面级组件保持 server component 只做组装（`output: export` 下仍会预渲染成静态 HTML，客户端组件必须自给自足）。
3. **旧 `ui/` 迁移期间保留**，作为可运行回退；`pnpm app` 改指 frontend，验收全 tab 回归后旧目录整体退役删除（含 vite 依赖）。
4. Rust 侧零改动——本次迁移只动 L6 层；dialogue Channel / done 事件 / 二进制资源路径均不变。



### 2.1 布局：左侧 tab 菜单 + 右侧内容展示

路由化后的落点：侧栏是 `app/layout.tsx` 的一部分，5 个 tab = 5 条路由（`usePathname` 高亮当前项），切换 = `router.push`；对话页的消息流内部滚动、其余页页体滚动的规则不变，由各页自身布局承担。

**窗口尺寸裁决：360×432 → 520×560。** 左侧栏固定吃掉 ~92px，360 宽下内容区只剩 ~270px，记忆列表 / 设置表单放不下；加高同理（对话输入框 + 页头）。侧栏 92px、内容区 ~428px。改动点：`tauri.conf.json` 的 width/height、`.app` 从纵向 flex 改为 `grid-template-columns: 92px 1fr`。

结构规则：

- **侧栏（固定，不滚）**：顶部是品牌区——迷你脸（复用现有 SVG，缩到 52px，仍然可点 = 打招呼）+ 名字 + 情绪点；下方 5 个 tab 项（图标 + 2 字标签，纵向排列，active 态高亮）。原 `hero` 的 caption（「平静地在窗台上待着」）取消——侧栏塞不下，情绪信息由情绪点颜色承载。
- **内容区（按 tab 切换）**：每页 = 页头（标题 + 右侧状态摘要）+ 页体。对话页页体不滚（消息流内部滚）；其余页页体 `.scroll` 独立滚动。各 tab 内容互不重排——切走再切回来，滚动位置和表单状态保持。
- 快捷键沿用：⌘1–⌘5 切 tab。

#### 整体骨架

```
┌──────────┬───────────────────────────────────────────┐
│ ┌──────┐ │                                           │
│ │ 迷你脸 │ │   内容区（右）                             │
│ │ 阿盆  │ │   ┌─────────────────────────────────────┐ │
│ │ 平静 ●│ │   │ 页头：标题            状态摘要 ● 本地  │ │
│ ├──────┤ │   ├─────────────────────────────────────┤ │
│ │ ▸对话 │ │   │ 页体                                │ │
│ │  表情 │ │   │   ┌─────────────────────────────┐   │ │
│ │  工具 │ │   │   │ card / list / form          │   │ │
│ │  记忆 │ │   │   └─────────────────────────────┘   │ │
│ │  设置 │ │   │   …（工具/记忆/设置页整体滚动）        │ │
│ └──────┘ │   └─────────────────────────────────────┘ │
└──────────┴───────────────────────────────────────────┘
   92px                    ~428px
```

#### 对话页（唯一不滚页体的页面）

```
│ ┌──────┐ │ 对话                          ● 本地就绪 │
│ │ 脸   │ │ ┌─────────────────────────────────────┐ │
│ │ 阿盆  │ │ │ （消息流，内部滚动）                   │ │
│ │ 平静 ●│ │ │        user │ 帮我看看今天有什么会     │ │
│ ├──────┤ │ │ 阿盆 │ 3 点有评审会，20 分钟前我提醒过。 │ │
│ │ ▸对话 │ │ │      └ 📅 日历直查                   │ │
│ │  表情 │ │ │ （预测卡 · 米色）阿盆 │ 我猜你明天……    │ │
│ │  工具 │ │ │              [猜对了][猜错了][别猜了] │ │
│ │  记忆 │ │ └─────────────────────────────────────┘ │
│ │  设置 │ │ ┌───────────────────────────────┬─────┐ │
│ └──────┘ │ │ 说点什么…                       │  ↑  │ │
│          │ └───────────────────────────────┴─────┘ │
```

#### 表情页（大脸进内容区，不再靠 hero 变形）

```
│ ┌──────┐ │ 表情                                     │
│ │ 脸   │ │         ┌───────────────────┐            │
│ │ 阿盆  │ │         │                   │            │
│ │ 平静 ●│ │         │    大脸 150px     │            │
│ ├──────┤ │         │  （实时状态）      │            │
│ │  对话 │ │         └───────────────────┘            │
│ │ ▸表情 │ │  瞬态表情： [平静][专注][困][被搭话]        │
│ │  工具 │ │            [不高兴][思考][得意]            │
│ │  记忆 │ │  形态演示： [破土][抽枝][成形][成景][休眠]   │
│ │  设置 │ │  ☝ 点脸 = 打招呼；演示不改真实状态           │
│ └──────┘ │                                          │
```

#### 工具页（原大按钮卡改为紧凑行列表，腾宽度给卡片）

```
│ ┌──────┐ │ 工具                                     │
│ │ 脸   │ │  动作                                     │
│ │ 阿盆  │ │  ┌─────────────────────────────────────┐│
│ │ 平静 ●│ │  │ 📄 今日日报 · 今天聊了什么        [运行]││
│ ├──────┤ │  │ 🗂 整理记忆 · 抽取·合并·消解      [运行]││
│ │  对话 │ │  │ 💬 主动说一句 · 看它想说什么      [运行]││
│ │  表情 │ │  └─────────────────────────────────────┘│
│ │ ▸工具 │ │  人格（新）                                │
│ │  记忆 │ │  ┌─────────────────────────────────────┐│
│ │  设置 │ │  │ 宪法要点 / 表达规范（折叠段）           ││
│ │      │ │  │ 当前语气：熟络 · 当前表情：平静 · S2成形 ││
│ │      │ │  │ ☝ 换模型只换表达，不换本体              ││
│ │      │ │  └─────────────────────────────────────┘│
│ │      │ │  感知层 / 日历 / 边界清单 / 它对你的理解     │
│ │      │ │  （四张卡，结构沿用现状，宽了以后更好排）      │
```

#### 记忆页（新）

```
│ ┌──────┐ │ 记忆                       事件 128 · 事实 34 │
│ │ 脸   │ │  总览（计数徽章行）                          │
│ │ 阿盆  │ │  [事件128] [事实34] [偏好12] [边界5]         │
│ │ 平静 ●│ │  [预测 7/12] [工具调用 41] [日历缓存 9]       │
│ ├──────┤ │                                           │
│ │  对话 │ │  ▾ 最近事件（kind 分组可折叠）                │
│ │  表情 │ │  │ dialogue    今天 14:02  帮我看看今天的会   │
│ │  工具 │ │  │ calendar_q  今天 14:02  命中日程查询      │
│ │  记忆 │ │  │ disagreement 昨天    没顺着你说……        │
│ │  设置 │ │  │ face_state  ……（灰显，派生事件）          │
│ └──────┘ │  ▾ 事实 / ▾ 偏好 / ▾ 最近一次预测 + 反馈状态  │
│          │  [整理记忆]  [忘记全部]（两段式确认）           │
```

#### 设置页（新）

顶部选择器 = 「本地 / 云端」双 tab；本地 tab 管安装与下载，云端 tab 管配置清单。

```
│ ┌──────┐ │ 设置                                     │
│ │ 脸   │ │  模型                                     │
│ │ 阿盆  │ │  ┌─────────────────────────────────────┐│
│ │ 平静 ●│ │  │ 顶部选择器：  [ 本地 ● | 云端 ]        ││
│ ├──────┤ │  └─────────────────────────────────────┘│
│ │  对话 │ │   ▼ 本地 tab ▼                           │
│ │  表情 │ │  已装模型（点卡片即切换，无需保存）           │
│ │  工具 │ │  ┌───────────────┐┌───────────────┐     │
│ │  记忆 │ │  │ ● qwen3.5:2b  ││   qwen3:4b    │     │
│ │  设置 │ │  │  0.8GB · 思维链可关│ 2.4GB · 16GB+ │  │
│ └──────┘ │  └───────────────┘└───────────────┘     │
│          │  状态：● sidecar 就绪 · 127.0.0.1:11435    │
│          │  可下载的推荐模型（并发 1 · 磁盘预警）        │
│          │  ┌─────────────────────────────────────┐│
│          │  │ gemma3:4b  3.3GB · 思维链无 · 16GB+    ││
│          │  │ [下载] ████████░░ 42% · 380/900MB 取消 ││
│          │  │ qwen3.5:4b  2.4GB · 思维链可关   [下载] ││
│          │  │ ▾ 更多（含 ⚠ 思维链不可关的模型）        ││
│          │  └─────────────────────────────────────┘│
│          │  ☝ 下载走 Ollama registry；ModelScope 镜像  │
│          │    （中国境内推荐）为后续增强项               │
│          │                                          │
│          │  ▼ 云端 tab（点「本地/云端」切换）▼          │
│          │  已配置的云端模型（点卡片即选中生效）          │
│          │  ┌─────────────────────────┬──┐          │
│          │  │ ● DeepSeek              │ ×│          │
│          │  │ deepseek-chat · api.deepseek.com │    │
│          │  │ key ••••4f2a · 测试 ✓             │    │
│          │  └─────────────────────────┴──┘          │
│          │  ┌─────────────────────────┬──┐          │
│          │  │   OpenAI                │ ×│          │
│          │  │ gpt-4o-mini · api.openai.com     │      │
│          │  └─────────────────────────┴──┘          │
│          │  [ ＋ 添加模型 ]  ← 打开添加弹窗            │
│          │                                          │
│          │  日志                                     │
│          │  ┌─────────────────────────────────────┐│
│          │  │ [应用日志 | sidecar日志]  [打开目录]     ││
│          │  │ 15:28:02 INFO sidecar ready on :11435 ││
│          │  │ （尾部 200 行 · 等宽 · 可见时 10s 轮询）  ││
│          │  └─────────────────────────────────────┘│
│          │  数据                                     │
│          │  [忘记全部…]（两段式确认 · 含边界与成长）      │
```

#### 添加云端模型弹窗（「＋ 添加模型」触发；编辑复用同一弹窗预填）

```
┌─────────────────────────────────────────┐
│  添加云端模型                        [×]  │
│                                         │
│  名称     [ 例：阿盆的 DeepSeek          ] │
│  类型     [ 自定义 OpenAI 兼容 ▾ ]        │
│           （选「OpenAI 官方」时端点锁定     │
│             api.openai.com 并隐藏此行；    │
│             「Claude（即将支持）」灰显）    │
│  接口地址  [ https://api.deepseek.com/v1 ]│
│  API Key  [ 明文输入                   ]  │
│  模型名   [ deepseek-chat             ]   │
│                                         │
│  ✓ 测试通过 · 1 token · 0.4s              │
│                                         │
│              [ 取消 ]  [ 保存 ]           │
└─────────────────────────────────────────┘
```

#### 云端 opt-in 警示模态（切云端必经，复用 onboarding 模态样式）

```
┌─────────────────────────────────────────┐
│  切换到云端模型？                          │
│                                         │
│  ⚠ 之后每一轮对话和记忆抽取的文本都会        │
│    发送到你填写的服务商，不再只留在本机。     │
│  · 人格卡与记忆库不出机器                  │
│  · 危机拦截、年龄门禁照常生效              │
│  · API Key 明文存在本机数据库              │
│                                         │
│           [ 还是本地 ]   [ 我知道了，切 ]   │
└─────────────────────────────────────────┘
```

- onboarding 模态不变（全屏覆盖层，盖住左右两栏）。


### 2.2 模型配置：一条配置总线

核心矛盾：4 个调用点各自 `from_env()`，没有共享状态。方案采用**进程级覆盖层**，不动调用点签名：

```rust
// llm.rs 新增
static LLM_OVERRIDE: OnceLock<RwLock<Option<LlmConfig>>> = OnceLock::new();

impl OllamaClient {
    /// 优先读覆盖层（用户 UI 设置），回落到环境变量（开发期习惯）。
    pub fn from_env() -> Self { /* override → env → default */ }
}
```

- 持久化：复用 schema v7 的 `settings` 表（不升 schema），键：
  - `llm_provider` = `ollama` | `cloud`
  - `llm_local_model`（默认 `qwen3.5:2b`）
  - `llm_active_id`：当前激活条目（本地 = `ollama:<tag>`；云端 = 配置条目 id）
  - `llm_cloud_entries`：云端配置列表 JSON 数组，每条 `{ id, name, kind, base_url, model, key }`（`kind` = `custom-openai` | `openai`）——**多实例存储**，同 meetily customInstanceId 模式；id 为时间戳派生随机串
- 热生效：`from_env()` 每次调用都读覆盖层，切换即时生效，**无需重启 sidecar**。

#### 2.2.1 本地模型管理（借鉴 meetily model-download-ux / OllamaModelList）

**点击选择，不是下拉框**——模型以卡片列表呈现（名称 + 大小 + 修改时间 + 标签），点卡片即切换（选中态高亮），切换动作 = 校验 + 写 settings + 更新覆盖层，一步完成（meetily 的 autoSaveOnSelect 模式），无独立「保存」按钮。

新命令：

- `llm_model_list_installed` → 代理 sidecar `GET /api/tags`（name/size/modified）；拉不到时前端回落**静态 fallback 清单**（meetily FALLBACK_MODELS 模式），并显示端点 chip `127.0.0.1:11435`
- `llm_model_download(tag)` → 起后台线程执行 `ollama pull <tag>`（复用 sidecar 二进制），NDJSON 进度解析后以 `llm:pull:progress` 事件推前端（status/percent/completed/total）；**并发上限 1**（队列语义，第二个下载请求返回 busy）；取消 = kill pull 进程
- `llm_model_remove(tag)` → `ollama rm`（有确认；当前激活模型不可删）
- `llm_catalog` → 内置推荐模型目录（见下），前端据此渲染「可下载」区

**推荐目录（catalog，编译内置）**——不照抄 Ollama 全站，只收经过验证的小模型，并打三类标签：

| 模型 | 大小 | 标签 |
|---|---|---|
| qwen3.5:2b | ~0.8GB | ✅ 当前默认 · 思维链可关（think:false） |
| qwen3.5:4b | ~2.4GB | 思维链可关 · 建议 16GB 内存 |
| gemma3:4b | ~3.3GB | 思维链无 · 建议 16GB 内存 |
| llama3.2:3b | ~2.0GB | 思维链无 · 中英混合稍弱 |
| deepseek-r1:1.5b | ~1.1GB | ⚠️ 思维链不可关（参照 qwen3:4b 教训，标记「不推荐做对话腿」） |

- 标签铁律来自 7872f7d 的教训：**思维链关不掉的模型污染正文**，catalog 必须显式标注 `think兼容`，不兼容的默认折叠在「更多」里并给警告。
- 下载源：v1 走 Ollama registry 原生 pull；**ModelScope 镜像列为增强项**（pull_model.sh 已有 ModelScope GGUF 兜底链路，UI 化时把「中国境内推荐」做进下载源选择，参照 meetily v8 的 catalog 映射方案）。
- 下载中状态：进度条 + `42% · 380MB/900MB` + 取消按钮；下载完成 toast + 列表自动刷新；磁盘不足（< 模型大小 ×1.2）下载前预警。

#### 2.2.2 远端模型配置（借鉴 meetily providerTabs / cloudModelCatalog / customInstanceId）

**顶部选择器 = 「本地 / 云端」双 tab。** 云端 tab 下是**已配置模型清单**（卡片列表，非表单）：

- 每张卡片 = 一条云端配置（名称 + 类型 + 模型名 + 端点摘要 + 脱敏 key），**点卡片即选中并生效**（流程：测试连接 → 写 settings → 更新覆盖层；失败内联报错不切换）；卡片带 × 删除（激活中的条目删时先弹确认）。
- 「＋ 添加模型」按钮 → **添加弹窗（模态）**：名称、类型（自定义 OpenAI 兼容 | OpenAI 官方）、接口地址（选 OpenAI 官方时锁定为 `https://api.openai.com/v1` 并隐藏）、API Key、模型名 → 「测试连接」（1 token 测试请求，内联结果）→ 「保存」落库回列表。编辑复用同一弹窗（预填、key 显示脱敏值）。
- **首次切换到任一云端条目必经 opt-in 警示模态**（每会话记住一次即可，见 §2.3）。
- 覆盖范围：自定义 OpenAI 兼容填 base_url 即覆盖 DeepSeek / Moonshot / GLM / Qwen 兼容模式 / OpenRouter；选 OpenAI 官方只需填 key，模型名下拉拉 `/v1/models`，失败回落静态清单。Claude 的 Messages API 不兼容，弹窗类型选项先灰显「Claude（即将支持）」，客户端属 M2.1 可选。

新命令（`llm_config_get/set` 保留）：

- `llm_cloud_entries_get` → 列表（key 一律脱敏）
- `llm_cloud_entry_save(entry)` → 新增/编辑（带 id 即编辑）；保存前不强测连接（测试在前端「测试连接」按钮显式触发，命令层只做格式校验：URL 合法、key 非空、模型名非空）
- `llm_cloud_entry_remove(id)` → 删除；删激活条目时联动回落本地默认模型
- `llm_config_set_active(id)` → 激活（云端条目激活前由命令层发一次 1-token 测试，失败拒绝并返回错误）

- API key 显示一律脱敏（`••••` + 末 4 位，meetily maskApiKey 模式）。

### 2.3 云端模型：ChatClient trait + 明示授权

新 trait 抽象两种传输（这是本次唯一的结构性重构）：

```rust
pub trait ChatClient: Send + Sync {
    fn chat(&self, msgs: &[Msg], temperature: f64, timeout: u64) -> Result<String, LlmError>;
    fn chat_stream(&self, msgs: &[Msg], temperature: f64, timeout: u64,
                   on_delta: &mut dyn FnMut(&str)) -> Result<String, LlmError>;
    fn is_up(&self) -> bool;
}
```

- `OllamaClient` 实现该 trait（现有逻辑平移）；新增 `CloudClient`：OpenAI 兼容 `/v1/chat/completions`，流式走 SSE（`data:` 行解析，与 parse_stream_line 同风格做纯函数 + 单测）。**自定义 / OpenAI 官方共用这一个客户端**（base_url 分别为用户自填 / `https://api.openai.com/v1`）；Claude 的 Messages API 不兼容，独立客户端属 M2.1 可选项。
- 调用点改造：`DialogueBackend`、`LocalExtractor`、persona eval、report 持有 `Arc<dyn ChatClient>`，由 `llm::current_client()` 工厂构造（读覆盖层）。抽取消歧规则不变——抽取腿对云端模型同样发结构化 JSON 指令，解析失败回落 RuleExtractor 的既有链路原样生效。
- **边界与安全（不可妥协）**：
  1. 云端为显式 opt-in：UI 默认 local；切云必须经过一个带红色警示的确认步骤（「对话与记忆抽取文本将上传到该服务商」），复用 onboarding 的模态样式。
  2. 人格卡与记忆库不出机器——上传的只是当轮对话文本与抽取指令，边界拦截（危机词、未成年门禁、「不准记录」）发生在模型调用**之前**，云端模式下同样生效，无需改动。
  3. 反谄媚 filter（generation.rs）是本地规则，云端回复照样过筛。
  4. API key 存 `settings` 表（SQLite 明文）。v1 接受此折衷，UI 注明；Keychain 迁移列入后续（与 P2-5 数据可携带一起裁决）。

### 2.4 人格展示：只读合成视图

不新增 Rust 状态，只加一个组合命令：

- `persona_snapshot` → `{ persona 卡（get_persona 现有内容）, 当前语气档（relationship::current_mode）, 当前表情（face_current）, 形态阶段（growth_status） }`
- UI（工具页「人格」卡片）：展示宪法要点 / 表达规范 / **当前生效**的语气档与表情，附一句「人格由 persona.json 定义，换模型只换表达、不换本体」。纯只读，不做编辑（改 persona 属于人格编辑器，另立项）。

### 2.5 记忆总览：一条只读命令

- `memory_overview(limit)` → 分区返回：
  - 计数区：events（按 kind 分组，含派生事件 face_state/disagreement/relationship_*）、facts、preferences、boundaries、predictions（命中率）、tool_calls、calendar_cache
  - 明细区：四类存储各取最近 `limit`（默认 20）条，时间倒序；predictions 附最近一条猜测与反馈状态
- UI（记忆 tab）：计数徽章 + 分区列表，只读；底部放「忘记全部」（forget_all，二次确认）与「整理记忆」入口（现有命令搬个入口）。
- 原则沿袭 plan-relationship 的克制：**只展示，不提供编辑**——记忆可看是信任，可改是作弊（边界清单除外，它已有删除入口）。

### 2.6 debug 日志：先有 logger，再有页面

- **接 logger**（前置，否则无日志可看）：引入轻量文件 logger（`simplelog` 或自写 20 行 WriteLogger），初始化在 `run()` setup 最前，落 `app_log_dir()/bonsai.log`，级别 INFO（debug 构建DEBUG），轮转按大小（>5MB 重的命名 `.1`）。
- sidecar 日志 `llm-sidecar.log` 已存在，一并纳入。
- 新命令：
  - `debug_log_tail(file, lines)` → 读 bonsai.log / llm-sidecar.log 尾部 N 行（默认 200）
  - `debug_open_log_dir` → opener 打开日志目录（用户自己翻全量）
- UI（设置页「日志」卡片）：两个日志源 tab + 尾部 200 行等宽字体展示 + 10s 轮询刷新（页面可见时才轮询）+「打开日志目录」按钮。

### 2.7 日历授权：验收即可

P2-2 已完整实现（授权弹窗由开关拨动触发、拒绝必须显式报错、`calendar_open_settings` 可跳系统设置）。M5 仅：真机走一遍「未授权 → 授权 → 拒绝后重授权」三态，核对状态行文案。

## 3. 分期落地

| 里程碑 | 内容 | 新命令 | 预估规模 |
|---|---|---|---|
| M0a | **前端迁移 Next.js**（§2.0）：脚手架 + 样式变量平移 + main.js 按域拆组件 + tauri.conf 接线 + 旧 ui/ 退役；行为视觉 1:1，Rust 零改动 | — | 前端重写 ~2800 行（多文件）+ 删 ~3100 行 |
| M0b | **布局壳**：侧栏（layout.tsx + 5 路由 + 迷你脸品牌区）+ 窗口 520×560（tauri.conf.json）+ 表情页大脸进内容区 | — | 前端 ~250 行 |
| M1 | 模型配置总线：覆盖层 + settings 持久化 + **本地模型卡片选择/下载（ollama pull + 进度事件）/删除 + 推荐目录** | llm_config_get / llm_config_set / llm_model_list_installed / llm_model_download / llm_model_remove / llm_catalog | Rust ~280 行 + 前端 ~220 行 |
| M2 | 云端模型：ChatClient trait 抽象 + CloudClient（SSE 流式）+ 云端配置清单（列表卡片 + 添加/编辑弹窗 + 删除）+ opt-in 警示 + 4 调用点改造 | llm_cloud_entries_get / llm_cloud_entry_save / llm_cloud_entry_remove / llm_config_set_active | Rust ~320 行 + 前端 ~200 行 |
| M2.1 | （可选）Claude Messages API 独立客户端 + pane | — | Rust ~120 行 |
| M3 | 记忆 tab：memory_overview + 页面 + 忘记全部入口 | memory_overview | Rust ~120 行 + 前端 ~150 行 |
| M4 | 人格卡片 + 日志：logger 接入 + persona_snapshot + 日志页 | persona_snapshot / debug_log_tail / debug_open_log_dir | Rust ~130 行 + 前端 ~130 行 |
| M5 | 日历授权真机验收 + 全部页面真机回归 + 更新 roadmap | — | 文档 |

M1→M2 有依赖（trait 在 M2），M3/M4 可与 M1/M2 并行。每里程碑 cargo check + 单测全绿 + `pnpm app` 真机过一遍再进下一个。

## 4. 测试计划（沿项目惯例，纯函数优先）

**M0a 迁移新增（前端首套测试基建，vitest + RTL）**：
- `lib/tauriInvoke.ts`：超时拒绝、正常透传（mock @tauri-apps/api）（~3 测）
- 事件收口 `tauriListen.ts`：注册/卸载/重复注册幂等（~3 测）
- 抽出的纯逻辑（键盘快捷键映射、日期格式化、消息分组等从 main.js 拆出时落 lib/ 并带测，~8 测）
- llmStore：provider 切换 / 下载进度快照 / active id 回落（~5 测）

**功能里程碑（Rust，cargo test）**：

- 覆盖层：override 存在时压过 env、清空回落、settings 读写往返（~4 测）
- CloudClient：SSE 行解析纯函数（content delta / done / error / keepalive，~5 测）；请求体构造（~2 测）
- pull 进度：NDJSON 进度行解析（percent/completed/total/完成/错误，~4 测）；并发上限 1（第二个请求 busy，~1 测）
- catalog：条目字段完整（tag/size/标签/think 兼容必填，~2 测）
- 配置校验：非法 provider / 空 key / 空 base_url / 空模型名拒绝（~4 测）；云端条目 CRUD（保存/编辑预填脱敏/删除激活条目回落本地默认，~4 测）；`llm_config_set_active` 激活前测试失败拒绝切换（~2 测）
- key 脱敏：mask 函数（长 key 留末 4 位、短 key 全遮、空值，~3 测）
- memory_overview：空库各计数为 0、kind 分组正确、limit 生效、派生事件计入 events（~4 测）
- persona_snapshot：冷/熟/顶嘴三档 + cold 状态字段正确（~3 测）
- debug_log_tail：tail 行数、文件缺失返回空不 panic（~2 测）
- 现有 162 测不许退化；预计新增 ~23 测

## 5. 风险与裁决记录

1. **key 明文存 SQLite** — v1 接受，UI 警示，随 P2-5 一起裁决 Keychain。
2. **云端模型质量不可控**（可能不守 JSON 抽取格式）— 既有 RuleExtractor 回落兜底，无数据丢失。
3. **from_env 改语义** — 名字不改（调用点少动），注释说明优先级：override > env > default。
4. **窗口从 360×432 改为 520×560** — 布局换侧栏的必要代价，见 §2.1；回退路径：若坚持小窗，侧栏可退化为 44px 纯图标列，但记忆/设置页可用性明显下降，不做首选。对话页快捷键 ⌘1–⌘5 需同步扩展两个新键位。
5. **不做的事**：人格编辑器、记忆编辑/伪造、云端默认开启。云端配置清单 v1 不做：分组/排序、同名去重提示、导入导出（随 P2-5 数据可携带裁决）、Claude 类型（M2.1）。
6. **模型下载**：并发上限 1（sidecar 单模型加载，多任务并行只互相抢 IO）；磁盘不足预警但不拦截（用户自负）；catalog 只收验证过的小模型、**思维链不可关的必须打 ⚠ 标签**（qwen3:4b 教训：污染正文）；ModelScope 下载源为后续增强，v1 不做。
7. **参考实现**：meetily（同机 `/Users/xuyi/Desktop/git/meetily`）——模型配置交互：model-download-ux v8（ModelScope catalog 映射）、OllamaModelList（卡片选择 + 搜索 + 进度）、cloudModelCatalog（fallback 清单 + maskApiKey）；**前端工程**：`frontend/` 整体（Next 14 + output:export + tauri.conf 接线）、`lib/tauriInvoke.ts`（15s 超时 fail-fast）、`lib/tauriListen.ts`、next.config 的 chunkLoadTimeout / watchOptions 调优。交互模式与工程模式均借鉴，代码不复用（其组件耦合 Followup 业务）。
8. **Next.js 迁移风险**（M0a）：
   - dev 冷启动慢 + ChunkLoadError → 照搬 meetily `chunkLoadTimeout: 600000` + `watchOptions.ignored`；
   - StrictMode 下流式 Channel 双订阅/事件双注册 → `reactStrictMode: false` + listen 统一收口 `tauriListen.ts`（safeUnlisten）；
   - `output: 'export'` 下误用 server-only 特性会构建失败 → 全部交互组件 `'use client'`，页面只做组装；
   - 迁移期双前端并存 → 旧 `ui/` 保留为回退，`pnpm app` 切指 frontend 后旧目录整体退役（同一 PR 不留死代码）；
   - bundle 体积增大 → 静态导出无运行时服务，接受；shadcn 按需引入不做全家桶。

## 6. 不改动清单

- 危机拦截 / 未成年门禁 / 「不准记录」的调用前位置 —— 云端模式下原样生效
- 对话管线顺序（危机 → 工具 → 模型）与 DialogueOutcome 协议
- sidecar 生命周期（仍只管 11435 本地服务；云端不经过 sidecar）
- schema 不升级（settings 表复用），M1–M5 全程 v7

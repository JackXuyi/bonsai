# AGENTS.md — bonsai 前端约束规范

> 本文件是项目约定文档，所有 AI 协作与人工开发在修改 `frontend/` 时必须遵守。
> 违反核心约束的代码不允许提交。

## 适用范围

- 主要作用于 `frontend/`（Next.js 14 + React 18 + TypeScript 5.7 + Tailwind CSS 3.4 + zustand 5）。
- `src-tauri/`（Rust）遵循 cargo clippy / 现有模块分层约定，不受本文件样式类条款约束。

## 核心约束（硬性，违反不允许提交）

1. **重复代码上限**：同一段代码（逻辑块 / JSX 块）在项目中重复出现 **2 次以上、且重复行数 > 10 行**，必须抽取为公共函数（`src/lib/`）、自定义 hook（`src/hooks/`）或公共组件（`src/components/`）。
   - 注意：相似 ≠ 相同。差异点应参数化（props / 入参），而不是靠复制粘贴微调。
2. **跨页面复用必须提取公共基础组件**：一个组件被 **2 个及以上页面**（`src/app/` 下不同路由）引用时，必须提取到 `src/components/` 下作为公共基础组件，禁止直接把页面内组件复制到另一个页面。
   - 页面私有组件就近放在所属 feature 目录（如 `src/components/settings/`），一旦发生跨页面复用立即上移为公共组件。
3. **优先复用已有组件**：动手新写任何组件 / 工具函数前，必须先检查 `src/components/`、`src/hooks/`、`src/lib/` 是否已有可复用实现；有则直接复用或扩展（加 props / 参数），不允许"换个地方再写一份"。
   - 扩展已有组件优先用可选 props 保持向后兼容，避免破坏现有调用方。
4. **状态管理**：需要跨组件 / 跨页面共享状态时引入 store，统一使用 **zustand 及其生态**（`persist`、`subscribeWithSelector` 等中间件按需引入）。
   - 组件内部的局部状态（开关、表单输入、loading）用 `useState`，能不上 store 就不上 store。
   - store 文件命名 `use{Name}Store.ts`，按领域拆分（一个领域一个 store），派生数据在组件内用 `useMemo` 计算，不落进 store。
   - 组件消费 store 时使用 selector 按字段订阅，避免整 store 订阅导致多余重渲染。
5. **样式统一走 Tailwind**：项目使用 TypeScript + Tailwind CSS + React，**非必要不手写样式**。
   - 禁止 inline `style={{...}}`（动态值除外，且需注释原因）；禁止新建零散 CSS 文件。
   - 确需全局/动态样式写入 `src/app/globals.css` 或现有样式文件，使用语义化 CSS 变量 / Tailwind theme token（`tailwind.config.ts`），不硬编码颜色和间距。

## 通用规范（业界惯例，结合本项目采纳）

### 组件

- 只写函数组件，不写 class 组件；一个文件一个主组件，文件名与组件名一致（PascalCase.tsx）。
- 组件文件超过 **~150 行**或 JSX 超过 **~80 行**时拆分子组件；业务逻辑超过 **~50 行**抽成自定义 hook（`use{Name}`，放 `src/hooks/`）。
- Props 接口命名为 `{ComponentName}Props` 并导出；在函数签名中解构 props；children 类型用 `React.ReactNode`。
- 变体用 `variant: 'primary' | 'ghost'` 这类判别式 props 表达，避免布尔 prop 堆叠（`isPrimary && !isSecondary && ...`）。
- 条件 className 用 `clsx` 拼接；Tailwind class 保持静态字符串，不用模板字符串动态拼类名。

### TypeScript

- strict 模式，禁止 `any`；props、回调、state 全部显式类型。
- 共享类型集中到 `src/types/` 或就近的领域文件，不在组件间重复声明。

### 状态与数据

- 状态放置原则：能用最近的共同祖先解决就不上 store；能用派生计算就不复制两份 state。
- Tauri invoke 调用集中在 `src/lib/` 封装层，不在叶子 UI 组件里散落裸调用。

### Tailwind 类组织顺序

按「布局（flex/grid/position）→ 尺寸（w/h）→ 间距（p/m）→ 文字（text/font）→ 视觉（bg/border/shadow）→ 交互（hover/focus）」排列，保持全库一致。

### 提交前自检

- `cd frontend && npx tsc --noEmit` 通过；`npm run test`（vitest）通过；改动涉及组件时跑 `npm run lint`。
- 新增组件 / hook 需附带最小测试（放 `frontend/tests/`，与现有测试结构一致）。

## 例外流程

确实无法满足上述约束时（如性能热路径、第三方库限制），在 PR / commit message 中说明原因与替代方案，经确认后豁免；不允许静默违反。

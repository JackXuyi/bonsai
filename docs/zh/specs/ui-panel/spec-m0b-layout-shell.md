# spec-m0b — 侧栏布局壳（左侧 tab 菜单 + 右侧内容）

> 依赖：M0a。目标：窗口 360×432 → 520×560；tab 从顶部胶囊改为左侧栏；新增「记忆」「设置」两个路由页（骨架，内容在 M1–M4 填充）。

## 1. 布局实现

- `app/layout.tsx`：`.app` 改 CSS Grid 两栏 `grid-template-columns: 92px 1fr`（globals.css 中 `.app` 类，非 inline）。
- 侧栏 `components/layout/Sidebar.tsx`：
  - 品牌区：迷你脸（`BonsaiFace size="sm"` 52px，仍可点 = poke）+ 名字「阿盆」+ 情绪行（情绪点颜色 + 文字）。**旧 hero 的 caption 取消**，情绪信息由情绪点承载。
  - 5 个导航项（图标 + 2 字标签纵排）：对话 `/` · 表情 `/face` · 工具 `/tools` · 记忆 `/memory` · 设置 `/settings`。当前项高亮：`usePathname()` 比对。
  - 页头 `components/layout/PageHeader.tsx`：标题（按路由映射）+ 右侧状态摘要（llm:ready → 「本地 · qwen3.5:2b」，后续 M1/M2 扩展 provider 文案）。
- 路由切换 `router.push`；`app/page.tsx`（对话）保持默认路由。
- 快捷键 ⌘1–⌘5：`hooks/useTabShortcuts.ts`（keydown 监听，metaKey+数字 → router.push；纯函数 `lib/shortcut.ts: routeForKeys(meta, key)` 带测）。
- 窗口尺寸：`tauri.conf.json` `windows[0]` width 360→520，height 460→560（外框含标题栏，内容高 532，侧栏全高）。

## 2. 各页滚动规则（沿 plan §2.1）

| 页 | 结构 | 滚动 |
|---|---|---|
| 对话 | 页头 + 消息流 + composer | 页体不滚，消息流内部滚 |
| 其余四页 | 页头 + 页体 | 页体 `.tab-body` 独立滚动 |
- 各页滚动位置/表单状态切走后保留：Next App Router 默认卸载非当前页——**对话页例外**，消息流状态需跨页保留 → 消息列表放 `chatStore`（zustand，M0a 的 uiStore 扩展：`messages[]`、pushMessage、replaceTyping），ChatPane 变纯渲染 + 订阅。其余页数据每次进页重查（本就是命令拉取，可接受）。

## 3. 骨架页占位

- `/memory`：页体渲染「记忆总览 · M3 建设中」占位卡。
- `/settings`：渲染「设置 · M1 建设中」占位卡。
- M1–M4 各 spec 直接替换各自占位，不动壳。

## 4. 工具页 / 表情页在新宽度下的调整

- 表情页：大脸进内容区常驻（不再靠 hero 变形放大），瞬态表情/形态演示按钮排其下——`app/face/page.tsx` 重排，`BonsaiFace size="lg"` 150px。
- 工具页：现有四卡宽度自适应即可，无结构改动；M4 会在其中插入人格卡（占位注释预留）。

## 5. 测试清单（~6 测）

1. `shortcut.ts`：⌘1–5 → 路由映射 / 非修饰键不触发（2）
2. `Sidebar`：pathname 高亮当前项（1）
3. `chatStore`：pushMessage / replaceTyping / 跨页保留（快照断言）（3）

## 6. 验收清单

- [ ] 520×560 窗口，侧栏 5 项点击 + ⌘1–5 全部可达，高亮正确
- [ ] 对话页聊天中切到设置再切回，消息仍在
- [ ] 对话流式正常（消息流内部滚动 + 回到底按钮）
- [ ] 表情页大脸 150px 常驻，演示按钮工作
- [ ] 记忆/设置占位页可达；旧三页内容无缺失
- [ ] `pnpm test` 全绿；截图对比 M0a 基线：三旧页仅布局差异、无样式回归

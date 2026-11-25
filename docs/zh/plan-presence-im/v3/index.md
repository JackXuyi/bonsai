# plan-presence-im v3 — 默认大肥鱼形象 · 素材与授权 · 叠在 v2 壳上

| 文档信息 | 内容 |
|---|---|
| 版本 | **v3**（叠在 [v2](../v2/index.md) 之上） |
| 日期 | 2026-11-25 |
| 状态 | **已被 [v4](../v4/index.md) 取代**（素材许可结论仍有效；去盆栽与可配置名见 v4） |
| 相对 v2 | 默认视觉从「盆栽」改为 **DeepSeek 社区大肥鱼形象**；补齐免费/无水印来源与合入边界 |
| 仍有效 | v2：主题换肤 · 聊天框右上角 ChatPet · 顶栏右侧只留 ⚙ |

---

## 0. 一句话

**默认 presence = 大肥鱼（chibi / 表情态）**；盆栽降为可选内置皮。素材只合入**无水印且许可可核验**的图；**禁止**把标注 `personal` / 未声明的图当默认随包分发。

人格文案权威仍是 `persona.json`（现名「阿盆」）——本版只改**默认皮肤/视觉**，是否改名另裁（见 Q1）。

---

## 1. 相对 v2 的变更表

| 项 | v2 | v3 |
|---|---|---|
| `DEFAULT_SKIN` | → `bonsai` | → **`fatfish`（大肥鱼表情皮）** |
| 盆栽经典 | 默认 | **可选**内置 |
| DeepSeek 小鲸 logo | 可选 / 曾默认 | **兜底 / 无表情态时的 fallback**（已在仓） |
| 「不抄鲸鱼娘 IP」 | v2 立场 | **撤销为产品默认视觉**；须合规合入 + NOTICE，非「无授权盗用」 |
| 主题默认 | `moss` 苔绿 | **建议默认改 `whale`（深蓝强调）**；`moss` 保留可选（见 §5） |

v2 的 T1–T3（顶栏收口 / ChatPet / 主题包）不变；**T4 默认皮**改为大肥鱼。

---

## 2. 网络素材检索结论（2026-11-25）

检索目标：免费、**无水印**、可作 ChatPet / 空态 / 表情槽的大肥鱼相关图。

### 2.1 可用源一览

| 来源 | 内容 | 水印 | 图片许可（公开标注） | 能否作**默认随包** |
|---|---|---|---|---|
| [dsh-meme-packs · 猫鲸](https://github.com/yyh-001/dsh-meme-packs/releases) | 64 张表情，CC0-1.0 | 无（原图分发） | **CC0-1.0** | **可（首选合入候选）** |
| [蓝色大肥鱼档案馆](https://fisharchive.cc/) / [llms.txt](https://fisharchive.cc/llms.txt) | 大量同人表情，开放 JSON | 站方称不加水印 | 多数 **CC BY-NC-SA 4.0**；部分投稿标 **CC0** | **仅 CC0 条目可默认可选入包**；NC 仅可选下载/用户导入 |
| [dafeiyu-bot 示例图](https://github.com/SAKURAfan1023/dafeiyu-bot/blob/main/THIRD_PARTY_NOTICES.md) | 4 张项目原创示例 | 无 | **CC0-1.0** | **可**（张数少，适合补槽/占位） |
| [dsh-im-style-fatfish `assets/faces`](https://github.com/byebyeway/dsh-im-style-fatfish) | 12 张回合表情（与 IM 插件同款） | 无 | 来自 `dafeiyu-001`；上游图库标 **`personal`**；NOTICE 要求自核条款 | **不可默认随包**；可「用户确认导入」 |
| [dsh-meme `dafeiyu-001`](https://cdn.jsdelivr.net/npm/dsh-meme@0.1.86/memes/dafeiyu-001/manifest.json) | 24 张经典大肥鱼 | 无 | manifest：`"license": "personal"` | **不可默认随包** |
| [Wikimedia · DeepSeek logo](https://commons.wikimedia.org/wiki/File:DeepSeek_logo.svg) | 官方蓝鲸标 | 无 | 文件侧 MIT/Expat 叙述 + **商标保留** | **可作非拟人兜底**（本仓已有 `deepseek-whale.png`） |
| 萌娘百科 / 各类立绘墙 | 全身立绘 | 不一 | 多为 CC BY-NC-SA 或未清 | **不默认可入包**；衬底仅用户自选 |

### 2.2 角色底层权利（必读）

社区共识（档案馆 / 画廊整理）：

- 原作意象：上善无形「溟月」等  
- 女仆大肥鱼二次设计：ZipZipPipe，常见标注 **CC BY-NC-SA 4.0**（署名 · **非商用** · 相同方式共享）

因此：即使单张投稿标 CC0，**角色设计层仍可能受 NC 约束**。产品若未来商用分发，须：

1. 默认包只用许可链清晰且允许再分发的子集；或  
2. 默认用官方鲸标 + 用户 opt-in 导入同人皮；或  
3. 书面风险接受（PR/commit 豁免，见 AGENTS §例外流程）。

本仓当前定位 **local-first 个人桌面**：v3 **推荐默认合入 CC0 表情子集**（体验贴近大肥鱼），并在设置·形象页固定 **署名与来源链接**；发布物附 `NOTICE` / `THIRD_PARTY_NOTICES`。

### 2.3 明确不采用

- 带站点角标/推广水印的转载站图  
- 未标许可的 QQ 群流传包  
- 仅「可预览不可再分发」的 CDN 热链当唯一来源（可作开发预览，合入须落盘 + 许可文件）

---

## 3. 默认形象方案（产品）

### 3.1 皮肤注册

```
BUILTIN_SKINS:
  fatfish   ★ DEFAULT   kind: image-set（表情槽）  大肥鱼
  deepseek              kind: image（单图）         官方小鲸 logo（fallback）
  bonsai                kind: svg                  盆栽·经典（可选）
  + custom…
```

- `DEFAULT_SKIN = "fatfish"`（替换现网 `deepseek` / 方案 v2 的 `bonsai`）。  
- `FALLBACK_IMAGE` 仍可指向官方小鲸，缺槽时兜底。

### 3.2 表情槽 ↔ FaceState（ChatPet）

优先做 **7 槽对齐现有 `FaceState`**（与自定义皮同一契约），素材从「可合入包」里挑语义最近的一张：

| FaceState | 语义 | 选图倾向（大肥鱼语汇） |
|---|---|---|
| `calm` | 常态 / 干饭 | 平静、端饭、待机 |
| `thinking` | 运行中固定 | 加载、思考、疑惑 |
| `focused` | 认真 | 加班、盯屏 |
| `sleepy` | 夜深 / 取消 | 睡觉、躺平 |
| `greeted` | 主动搭话 | 打招呼、比心 |
| `displeased` | 失败 / 边界 | 生气、无语 |
| `proud` | 交付 | 得意、点赞 |

若合入的是 **猫鲸 CC0** 等非 1:1「经典 24 张」包：仍按上表人工映射，不在 UI 声称「官方 dafeiyu-001」。

若未来用户导入 **im-style 12 脸**（personal）：映射表可扩展为 hello/think/…（见 v1），与 FaceState 多对一。

### 3.3 ChatPet / 空态 / 衬底

沿用 [v2 §5](../v2/index.md#5-chatpet聊天框右上角互动头像)：

- 右上角互动头像：当前皮的对应槽位图（圆裁）  
- 空态大脸：同一皮的 `calm`（或 `greeted`）大图  
- 可选淡衬底：仅用户开启；默认素材用 **已合入 calm 大图极淡**，不用未授权立绘墙图  

### 3.4 Wireframe（默认皮变更后）

```
┌─ ChatChrome ─────────────────────────────┐
│ ≡                                      ⚙ │
├─ chat-stage ─────────────────────────────┤
│                                    ╭──╮  │
│                                    │🐟│  │  ChatPet = 大肥鱼表情
│                                    │想│  │  streaming → thinking
│                                    ╰──╯  │
│              …消息…                       │
│  ┌─ Composer ─────────────────────────┐  │
│  └────────────────────────────────────┘  │
└──────────────────────────────────────────┘

设置 · 形象
  ★ 大肥鱼（默认）  · 小鲸 logo  · 盆栽  · 自定义
  [素材署名] 猫鲸 CC0 / … → 链接
```

---

## 4. 合入与工程切片（素材相关）

| 切片 | 内容 | 验收 |
|---|---|---|
| **A1 许可锁定** | 选定默认包（推荐：猫鲸 CC0 Release ZIP 或 CC0 子集）；写 `frontend/public/skins/fatfish/NOTICE.md`（作者、链接、许可全文指针、获取日期、文件清单哈希） | NOTICE 与文件一一对应 |
| **A2 落盘** | `calm.webp` 等 7 槽（或 jpg）；无水印；长边建议 512–1024；ChatPet 用 cover 圆裁 | 设置预览 / 空态 / Pet 同源 |
| **A3 注册** | `skins.tsx`：`fatfish` image-set；`DEFAULT_SKIN="fatfish"`；盆栽保留 | 冷启默认大肥鱼 |
| **A4 UI 文案** | 形象卡名称「大肥鱼」；desc 写清社区同人非官方；链到署名 | 不冒充 DeepSeek 官方 IP |
| **A5（可选）导入器** | 设置内「导入 DSH 表情包 ZIP」（personal/NC 包用户自担） | 默认包不含 personal 图 |

与 v2 切片合并顺序建议：**T1 顶栏 → T2 ChatPet → A1–A4 默认肥鱼 → T3 主题（whale 默认）→ T4 删除**（原「改盆栽」作废）。

质量门：前端 tsc / 相关 test / lint；`NOTICE` 进仓；**不**把 `personal` 图打进默认资源目录。

---

## 5. 主题色（随默认形象微调）

大肥鱼视觉常见深蓝（约 `#4D6BFE` / `#5D78FE` 一带）。v3 建议：

| id | 角色 |
|---|---|
| `whale` | **新默认**：深/亮蓝强调 + 冷灰纸或浅底；对齐鲸形象 |
| `moss` | 原苔绿，可选（盆栽皮搭配） |
| `ink` / `clay` / `paper` | 同 v2 |

换肤仍只改 CSS 变量包；脸素材不随主题重染色（避免毁表情）。

---

## 6. 待决问题

| # | 问题 | 推荐默认 |
|---|---|---|
| Q1 | 人格显示名是否从「阿盆」改为「大肥鱼」？ | **先不改 persona.json**；仅皮肤名「大肥鱼」。改名另开人格案 |
| Q2 | 默认合入哪套图？ | **猫鲸 CC0**（或档案馆明确 CC0 的静图子集）；体验不够再评估风险接受后的 personal 包 |
| Q3 | 主题默认是否切 `whale`？ | **是** |
| Q4 | NC / personal 包？ | **不进默认包**；设置可选导入 + 明示许可 |
| Q5 | 角上头像位置 | 仍 **右上**（v2） |

---

## 7. 风险

| 风险 | 缓解 |
|---|---|
| 误把 `personal`/`NC` 当可商用默认 | A1 门禁；CI 或 PR checklist 核对 NOTICE |
| 用户以为官方形象 | 文案「社区同人 · 非 DeepSeek 官方」 |
| CC0 包画风与「经典大肥鱼」不一致 | 设置保留导入经典包；或后续自绘/委托可再分发皮 |
| 商标 | 官方鲸标仅作模型/兜底识别，不声称官方背书 |

---

## 8. 目录

- [§0 一句话](#0-一句话)
- [§1 相对 v2](#1-相对-v2-的变更表)
- [§2 素材检索](#2-网络素材检索结论2026-11-25)
- [§3 默认形象](#3-默认形象方案产品)
- [§4 切片](#4-合入与工程切片素材相关)
- [§5 主题](#5-主题色随默认形象微调)
- [§6 待决](#6-待决问题)
- [§7 风险](#7-风险)

### 关键外链（检索快照）

- 档案馆说明：https://fisharchive.cc/llms.txt  
- 猫鲸等图库市场：https://dshcrate.com/p/yyh-001/dsh-meme-packs  
- dafeiyu-001 manifest（personal）：https://cdn.jsdelivr.net/npm/dsh-meme@0.1.86/memes/dafeiyu-001/manifest.json  
- IM 12 脸 NOTICE：https://github.com/byebyeway/dsh-im-style-fatfish/blob/main/NOTICE.md  
- 官方鲸标：https://commons.wikimedia.org/wiki/File:DeepSeek_logo.svg  

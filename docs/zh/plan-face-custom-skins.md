# 方案：表情成套自定义皮肤（图片上传版）

> **状态**：v0.2 已实施（M0–M2 落地，M3 真机验收待做）
> **日期**：2026-11-23
> **前置**：皮肤系统已落地（commit 5d456c6：内置 deepseek 图片皮肤 + bonsai SVG 皮肤 + 设置页选择卡）

## 1. 背景与目标

当前皮肤系统只有两个内置皮肤：DeepSeek 小鲸（单张静态图）与盆栽经典（SVG 成长形态）。用户无法用自己喜欢的形象。

**目标**：把「表情」做成一套完整的自定义方案——用户为每个表情状态上传对应图片，组成一套自定义皮肤；上传后侧栏迷你脸与表情页大脸即时换装。

**非目标**（本期不做）：

- 不做图片编辑/裁剪（CSS `object-fit: contain` 兜底展示）；
- 不做成长形态（stage 0–3）的图片变体——自定义皮肤是静态形象，成长线只对盆栽 SVG 皮肤生效；
- 不做 dormant 专属图——统一走 CSS 降饱和滤镜（现有实现）；
- 不做皮肤市场/导入导出（P2-5 数据可携带时一并考虑）。

## 2. 现状代码事实（方案的全部锚点）

| 接入点 | 位置 | 事实 |
| --- | --- | --- |
| 表情状态集合 | `frontend/src/lib/types.ts:15` | `FACE_STATES` 7 态：calm / focused / sleepy / greeted / displeased / thinking / proud；`uiStore.setFaceState` 白名单校验 |
| 帧动画 | `useFaceFrame` | frame 0/1，600ms 翻转，驱动 SVG 位移帧与图片皮肤的 `data-frame` 轻浮动 |
| 皮肤注册表 | `frontend/src/components/face/skins.tsx` | `FACE_SKINS`（内置两枚）+ `makeImageSkin`（`<img>` + `data-skin` + `data-frame`/`data-dormant`）+ `faceSkinById` |
| 皮肤选择持久化 | `frontend/src/lib/stores/faceSkinStore.ts` | zustand persist，localStorage key `bonsai-face-skin`，默认 `deepseek` |
| 渲染出口 | `frontend/src/components/face/PokeFace.tsx` | 侧栏 52px 与表情页 150px 唯一出口；按钮挂 `data-skin`（光晕/涟漪随皮肤着色）；皮肤 `Component` 契约 |
| 选择卡 | `frontend/src/components/settings/FaceSkinCard.tsx` | 内置皮肤选择 + 预览（stage 2 静帧） |
| 文件对话框先例 | `src-tauri/src/filetools.rs:709` | `fs_pick_directory`：rfd `AsyncFileDialog` 在 Tauri 命令的 tokio 上下文安全，**文件字节不过前端 IPC** |
| 应用数据目录先例 | `src-tauri/src/lib.rs:89` 等 | `app.path().app_data_dir()` 已在 db/report/summary 使用 |

## 3. 皮肤数据模型

### 3.1 槽位 = 表情状态

一套自定义皮肤 = 7 个表情槽位（与 `FACE_STATES` 一一对应）+ 皮肤元数据：

```
{
  id: "c-k3f9q2",          // Rust 生成，见 §5.2 命名规则
  name: "我的猫",           // 用户命名（显示用，与 id 解耦）
  created_at: 1764000000,
  slots: {
    calm: "calm.png",       // 相对文件名；缺槽 = null
    greeted: "greeted.webp",
    ...                     // 7 态
  }
}
```

### 3.2 渲染优先级（图片皮肤）

对每个渲染帧，按顺序取第一命中：

1. **槽位图**：`slots[faceState]` 存在 → 显示该图；
2. **默认图**：`slots.calm`（calm 是必填槽，建皮肤时必须先传）；
3. **内置兜底**：DeepSeek 小鲸静态图（保证任何状态都有图，绝不白屏）。

### 3.3 动画与状态映射（复用现有机制，零新增）

| 维度 | 处理 |
| --- | --- |
| frame 0/1 | `data-frame` 驱动 CSS `translateY(-3px)` 轻浮动（已有） |
| dormant | `data-dormant` 驱动 CSS `grayscale(0.7) opacity(0.8)`（已有，不上传专属图） |
| poke / greeted | `.face.poked` 波纹 + `data-state` 弹跳（CSS 已有，皮肤无关） |
| stage / dayIndex | 忽略（静态形象不参与成长形态线） |

## 4. 存储设计

### 4.1 文件布局（Rust 管理，零 schema 变更）

```
app_data_dir()/face_skins/
  c-k3f9q2/
    manifest.json        # §3.1 结构
    calm.png             # 槽位图片，保留原始扩展名
    greeted.webp
  c-8x2m0d/
    manifest.json
    calm.png
```

选中的皮肤 id 仍由前端 persist 存储（`skinId` 值域扩展为自定义 id），后端不存偏好、settings 表不动。

### 4.2 图片如何进 `<img src>`：asset protocol（主选）

Tauri 2 asset protocol：`convertFileSrc(app_data_dir()/face_skins/...)` 生成 `asset://` URL 直接喂 `<img>`，零 IPC、零内存缓存膨胀。

改动点（评审时核实）：

- `tauri.conf.json` → `app.security.assetProtocol`：`enable: true`，`scope` 精确限定 `$APPDATA/face_skins/**`（不放开整个 app_data_dir）；
- 前端 `convertFileSrc` 来自 `@tauri-apps/api/core`（已在依赖树）。

备选（若 asset protocol 在真机出问题）：Rust 命令返回 base64 data URL + 前端 Map 缓存（2MB 图 → ~2.7MB/张，7 槽 ≈ 19MB 内存上限，可接受但不优先）。

## 5. Rust 命令面（新文件 `src-tauri/src/faceskins.rs`）

### 5.1 命令清单（4 枚，全走 rfd/文件系统，字节不过 IPC）

| 命令 | 职责 | 返回 |
| --- | --- | --- |
| `face_skin_list()` | 读 face_skins/ 下全部 manifest（目录损坏容错：跳过坏目录，不报错） | `Vec<FaceSkinManifest>` |
| `face_skin_create(name)` | 生成 id + 建目录 + 写空 manifest（无槽位） | manifest |
| `face_skin_pick_and_import(skin_id, slot)` | rfd 弹文件选择器 → 校验（§5.3）→ 拷贝进 `face_skins/<id>/<slot>.<ext>` → 更新 manifest | manifest |
| `face_skin_delete(skin_id)` | 删目录（递归，限 face_skins 内）；manifest 返回 | () |

槽位清除 = 再上传同槽位时选择取消？——不，单槽清除由前端「已上传槽位点 ×」调 `face_skin_pick_and_import` 的姊妹命令 `face_skin_clear_slot(skin_id, slot)`（共 5 枚命令，删文件 + manifest 置 null）。

### 5.2 安全规则

| 规则 | 实现 |
| --- | --- |
| skin_id 命名 | Rust 生成：`c-` + 8 位随机 alnum；**用户输入只进 name（manifest 内容），永不进路径** |
| 路径校验 | 所有文件操作先 `skin_id` 白名单校验（`^c-[a-z0-9]{8}$`）+ canonicalize 后必须落在 `face_skins/` 内（复用 filetools 的 component 前缀匹配思路，防 `../` 逃逸） |
| 格式校验 | magic bytes：PNG `\x89PNG`、JPEG `\xFF\xD8\xFF`、WebP `RIFF....WEBP`；**不信扩展名**，magic 不符 → `face_bad_image` 结构化错误 |
| 大小上限 | 单文件 ≤ 2MB（读取文件元数据后拒绝，不读入内存再拒） |
| 数量上限 | 皮肤 ≤ 10 套（manifest 计数，超限 `face_skins_max`） |
| 隐私红线 | 上传图片只落盘 + 渲染，**永不进记忆表、永不进 prompt、永不发云端**（纯本地外观资源） |

### 5.3 失败语义（对齐项目红线）

所有失败返回 `face_<reason>: 中文说明` 结构化字符串（对齐 `fs_roots_max` 先例），绝不静默成功、绝不返回空结果冒充成功。

## 6. 前端改动

### 6.1 skins.tsx 注册表重构

```ts
// FaceSkin 扩展：kind 区分渲染管线
interface FaceSkinBase { id: string; name: string; desc: string; custom?: boolean }
interface BuiltinSvgSkin extends FaceSkinBase { kind: "svg"; Component }        // 盆栽
interface ImageSkin     extends FaceSkinBase { kind: "image"; image?: string }  // 内置 deepseek：单图
interface CustomSkin    extends FaceSkinBase { kind: "custom"; manifest }       // 自定义：按槽位解析

// resolveSkinView(skin, state) → { type: "img", src } | { type: "svg", Component }
// 渲染优先级见 §3.2；faceSkinById 改为 (id, customSkins) 双表查找
```

`faceSkinStore` 扩展：`customSkins: FaceSkinManifest[]`（启动时 `face_skin_list()` 灌入）+ `refreshCustomSkins()` + `skinId` 值域放宽到 string；选中 id 失效时 `faceSkinById` 兜底回默认（已有逻辑）。

### 6.2 FaceSkinCard → SkinManager（设置页）

卡片内三区（沿用 card/card-head 结构）：

1. **皮肤列表**：内置 2 枚 + 自定义 N 套，单选（现 FaceSkinCard 逻辑平移）；
2. **自定义皮肤管理**：「新建皮肤」输入名称 → 列表项展开 7 槽位网格——每槽显示已传图缩略图 / 空态虚线占位，点「上传」调 `face_skin_pick_and_import`，已传槽位带 × 清除；皮肤行尾「删除」（confirm 后调 `face_skin_delete`）；
3. **说明文案**：格式/大小上限 + 隐私声明（「图片只保存在本机，不会上传」）。

calm 槽位标「必传」（渲染兜底依赖它，§3.2）。

### 6.3 组件与约束对齐（AGENTS.md）

- 上传/管理 UI 全部就近放 `src/components/settings/`；invoke 封装集中进 `src/lib/commands.ts`；类型进 `src/lib/types.ts`；
- 槽位网格 >10 行 JSX → 拆 `SkinSlotGrid` 子组件；新增 `tests/FaceSkin.test.tsx` 用例：自定义皮肤列表渲染、槽位上传状态切换、缺槽兜底回 calm、选中失效回退默认。

## 7. 里程碑（预计 ~1.5 周）

| 里程碑 | 内容 | 验收 |
| --- | --- | --- |
| M0 Rust 存储内核 | `faceskins.rs`：目录/manifest 读写 + id 生成 + 路径校验 + magic bytes 校验 + 5 命令；单测覆盖 id 正则、路径逃逸反例、magic 反例、manifest 往返 | `cargo test --lib` 全绿 + clippy 干净 |
| M1 注册表重构 | skins.tsx 三类皮肤管线 + store 扩展 + asset protocol scope 配置；PokeFace/FaceSkinCard 平移不破坏内置皮肤 | 内置两皮肤回归全绿 |
| M2 管理界面 | SkinManager 三区 UI + commands 封装 + 上传/清除/删除流程 + 单测 | 真机：新建皮肤 → 上传 7 图 → 即时换装 → 删除回退 |
| M3 收尾 | 全量门禁 + 文档回写（本文件 §10 实施记录） | tsc/vitest/lint/cargo 全绿，分组提交 |

依赖顺序：M0 无前端依赖可先行；M1 是 M2 的前置；全程零 schema 变更、零 TCC、零 npm 新包。

## 8. 验收清单（真机）

1. 新建皮肤「我的猫」→ 上传 calm.png → 侧栏与表情页立即显示猫图（未传槽位回退 calm 图）；
2. 补齐 greeted/proud 等槽位 → 对应表情触发时（如问好、表扬）切换到对应图；
3. 传 >2MB 文件 / 改扩展名的假 PNG → 结构化报错，manifest 不变；
4. 删除选中中的皮肤 → 自动回退默认 DeepSeek，不白屏；
5. 重启应用 → 自定义皮肤仍在（asset protocol scope 生效）；断网不影响（纯本地）；
6. 手改 face_skins 目录外文件路径攻击（`skin_id = "../x"`）→ 拒绝且审计留痕。

## 9. 开放问题（均附建议默认值）

| # | 问题 | 建议 |
| --- | --- | --- |
| 1 | 皮肤数量上限 10 够不够 | 够；超限场景留给数据可携带（P2-5） |
| 2 | 是否支持 GIF 动图 | 本期不做（magic 校验只放 PNG/JPEG/WebP）；GIF 需帧率控制，等有真实诉求再加 |
| 3 | dormant 是否提供专属图 | 不做，CSS 滤镜统一（省 7 槽心智负担） |
| 4 | 内置 DeepSeek 皮肤是否允许「覆盖上传」 | 不允许——内置只读，自定义基于空白模板 |
| 5 | asset protocol 不可用时的降级 | base64 data URL + 内存缓存（§4.2 备选），真机验收 M2 时决定 |

## 10. 实施记录（2026-11-23）

**Commits**：`0cc4025`（M0 存储内核）→ `dbbd888` + `9c26bec`（M1 注册表重构；补提含 SkinSlotGrid 组件 + tauri `protocol-asset` feature——assetProtocol 配置生效的必要条件）→ `a270146`（M2 管理界面）。

**已落地（与方案的差异标注）**

- `faceskins.rs`（~470 行含单测）：5 命令 + manifest BTreeMap + `c-`+8 位 base36 id（时间+pid 熵）；id 正则白名单 + canonicalize 界内校验；magic bytes（PNG/JPEG/WebP）；≤2MB 前置拒绝、≤10 套；list 坏目录容错跳过；排序 created_at(ms) 并列回退 id。
- 前端三管线：`FaceSkin` 判别联合（svg/image/custom）替代原 Component 单契约；`resolveSkinImage` 渲染优先级（槽位图 > calm > FALLBACK_IMAGE）；`resolveCustom` 返回 `string | undefined`（baseDir 未就绪回退，比方案更细）。
- asset protocol：`tauri.conf.json` scope `$APPDATA/face_skins/**` + Cargo.toml `protocol-asset` feature（补提提交）；`convertFileSrc` 解析 URL；`faceSkinStore` 扩展 `customSkins`/`baseDir`（partialize 只持久化 skinId），`ensureFaceSkinsLoaded` 懒加载一次。
- 管理界面：FaceSkinCard 三区 + `SkinSlotGrid`（7 槽缩略图/上传/× 清除）+ `skinSlots.ts` 槽位常量；删除走 `window.confirm`；错误 toast 只取冒号后中文说明；取消选择（face_pick_cancelled）不弹错。

**偏差两条（已记档）**

1. 单槽清除命令命名为 `face_skin_clear_slot`（方案 §5.1 表格漏列序号，语义一致）。
2. created_at 秒级改毫秒 + id 并列排序（同秒创建排序不稳定，单测抓住）。

**门禁**：cargo test --lib **451 全绿**（faceskins 7 例）+ clippy 对 faceskins 零警告；`tsc --noEmit` 干净；vitest **140 过**（FaceSkin 18 例：内置回归 4 + 渲染优先级 7 + 管理流程 7）；next lint 无警告。

**待真机验收（M3）**

1. 新建皮肤 → rfd 弹窗上传 calm → 侧栏/表情页立即显示（asset:// URL 加载）；
2. 传 >2MB / 假 PNG（改扩展名）→ 结构化 toast 报错，manifest 不变；
3. 删除选中皮肤 → 回退 DeepSeek 不白屏；重启后自定义皮肤仍在；
4. 手工构造 `skin_id="../x"` invoke → `face_bad_id` 拒绝；
5. 表情状态全量切换（7 态）→ 槽位图正确优先于 calm 兜底。

## 变更记录

| 版本 | 日期 | 说明 |
| --- | --- | --- |
| v0.1 | 2026-11-23 | 初稿：7 槽位模型、渲染优先级、asset protocol 存储、5 命令 Rust 面、M0–M3 里程碑 |
| v0.2 | 2026-11-23 | M0–M2 实施落地（0cc4025 / dbbd888 / 9c26bec / a270146），门禁全绿；M3 真机验收待做 |

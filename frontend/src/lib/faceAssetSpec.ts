/**
 * 表情素材规格（与 src-tauri faceskins.rs / 默认大肥鱼 1024² 对齐）。
 * 展示侧：正方形框 + object-fit:contain，完整显示整图。
 */
export const FACE_ASSET_SPEC = {
  /** 宽高比 */
  aspect: "1:1" as const,
  minSidePx: 256,
  maxSidePx: 1024,
  recommendSidePx: 512,
  maxBytes: 2 * 1024 * 1024,
  /** ChatPet 展示边长（CSS .face-md） */
  displayMdPx: 96,
  /** 空态 / 设置预览（CSS .face-lg） */
  displayLgPx: 128,
} as const;

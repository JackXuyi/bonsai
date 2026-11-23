// 表情槽位顺序常量（与 src-tauri/src/faceskins.rs SLOTS 对齐，
// calm 在首位——渲染兜底依赖它，UI 上标必传）。
export const SLOTS_ORDER = [
  "calm",
  "focused",
  "sleepy",
  "greeted",
  "displeased",
  "thinking",
  "proud",
] as const;

export type FaceSlot = (typeof SLOTS_ORDER)[number];

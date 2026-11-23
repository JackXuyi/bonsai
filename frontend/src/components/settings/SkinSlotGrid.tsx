"use client";

// 形象卡 · 槽位网格（plan-face-custom-skins.md §6.2）：一个自定义皮肤的
// 7 个表情槽位——已传显示缩略图 + 清除 ×，未传显示虚线占位 + 上传按钮。
// 事件回调由 FaceSkinCard 注入（本组件不做 invoke，保持纯展示 + 动作转发）。
import { SLOTS_ORDER } from "./skinSlots";
import { convertFileSrc } from "@tauri-apps/api/core";

export interface SkinSlotGridProps {
  skinId: string;
  /** slot → 槽位文件名（仅已上传） */
  slots: Record<string, string>;
  /** asset:// URL 解析（baseDir 未就绪时为 undefined，缩略图隐藏） */
  resolveSrc?: (skinId: string, file: string) => string | undefined;
  onUpload: (slot: string) => void;
  onClear: (slot: string) => void;
}

export function SkinSlotGrid({ skinId, slots, resolveSrc, onUpload, onClear }: SkinSlotGridProps) {
  return (
    <div className="skin-slot-grid" data-testid={`slot-grid-${skinId}`}>
      {SLOTS_ORDER.map((slot) => {
        const file = slots[slot];
        const src = file && resolveSrc ? resolveSrc(skinId, file) : undefined;
        return (
          <div key={slot} className="skin-slot" data-testid={`slot-${slot}`} data-filled={file ? "true" : "false"}>
            {src ? (
              /* eslint-disable-next-line @next/next/no-img-element -- 本地 asset 资源，同 skins.tsx 豁免理由 */
              <img className="skin-slot-img" src={src} alt={slot} draggable={false} />
            ) : (
              <span className="skin-slot-empty">{slot}</span>
            )}
            {file ? (
              <button
                type="button"
                className="skin-slot-btn"
                data-testid={`slot-clear-${slot}`}
                title="清除该表情图"
                onClick={() => onClear(slot)}
              >
                ×
              </button>
            ) : (
              <button
                type="button"
                className="skin-slot-btn"
                data-testid={`slot-upload-${slot}`}
                title="上传该表情图片"
                onClick={() => onUpload(slot)}
              >
                上传
              </button>
            )}
          </div>
        );
      })}
    </div>
  );
}

/** convertFileSrc 的局部转发（便于后续统一换协议实现）。 */
export function assetSrc(path: string): string {
  return convertFileSrc(path);
}

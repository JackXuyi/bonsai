"use client";

// 设置页 · 形象卡：脸部皮肤选择（纯前端偏好，zustand persist）。
// 预览画 stage 2 成形态，切换即时生效（侧栏 + 表情页共用 PokeFace）。
import { FACE_SKINS, faceSkinById } from "@/components/face/skins";
import { useFaceSkinStore } from "@/lib/stores/faceSkinStore";
import type { FaceSkinId } from "@/components/face/skins";

export function FaceSkinCard() {
  const skinId = useFaceSkinStore((s) => s.skinId);
  const setSkin = useFaceSkinStore((s) => s.setSkin);
  const active = faceSkinById(skinId);

  return (
    <div className="card" data-testid="face-skin-card">
      <div className="card-head">
        <span className="card-title">形象</span>
        <span className="card-sub">盆栽的样子，选一个顺眼的</span>
      </div>
      <div className="skin-options" role="radiogroup" aria-label="形象选择">
        {FACE_SKINS.map((skin) => {
          const Preview = skin.Component;
          const selected = skin.id === skinId;
          return (
            <button
              key={skin.id}
              type="button"
              role="radio"
              aria-checked={selected}
              data-testid={`skin-${skin.id}`}
              className={`skin-option${selected ? " on" : ""}`}
              onClick={() => setSkin(skin.id as FaceSkinId)}
            >
              <span className="skin-preview" aria-hidden="true">
                <Preview state="calm" frame={0} stage={2} dormant={false} seed={1} />
              </span>
              <span className="skin-meta">
                <span className="skin-name">{skin.name}</span>
                <span className="skin-desc">{skin.desc}</span>
              </span>
            </button>
          );
        })}
      </div>
      <p className="skin-note">当前形象：{active.name}。形象只是外观，阿盆的记忆和成长不受影响。</p>
    </div>
  );
}

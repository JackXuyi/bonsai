"use client";

// 设置页 · 形象卡：脸部皮肤选择（内置皮肤 + 自定义皮肤，M2 扩展槽位管理，
// plan-face-custom-skins.md §6.2）。预览即所见，切换即时生效。
import { useEffect } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { BUILTIN_SKINS, SkinPreview, faceSkinById, type FaceSkin } from "@/components/face/skins";
import { ensureFaceSkinsLoaded, useFaceSkinStore } from "@/lib/stores/faceSkinStore";

export function FaceSkinCard() {
  const skinId = useFaceSkinStore((s) => s.skinId);
  const customSkins = useFaceSkinStore((s) => s.customSkins);
  const baseDir = useFaceSkinStore((s) => s.baseDir);
  const setSkin = useFaceSkinStore((s) => s.setSkin);

  useEffect(() => {
    ensureFaceSkinsLoaded();
  }, []);

  const resolveCustom = (sid: string, file: string) =>
    baseDir ? convertFileSrc(`${baseDir}/face_skins/${sid}/${file}`) : undefined;

  // 选中 id 失效（皮肤被删 / persist 残留）→ 兜底内置默认
  const skin = faceSkinById(skinId, customSkins) ?? faceSkinById("deepseek")!;
  const all: FaceSkin[] = [
    ...BUILTIN_SKINS,
    ...customSkins.map((m) => faceSkinById(m.id, customSkins)!),
  ];

  return (
    <div className="card" data-testid="face-skin-card">
      <div className="card-head">
        <span className="card-title">形象</span>
        <span className="card-sub">盆栽的样子，选一个顺眼的</span>
      </div>
      <div className="skin-options" role="radiogroup" aria-label="形象选择">
        {all.map((s) => {
          const selected = s.id === skinId;
          return (
            <button
              key={s.id}
              type="button"
              role="radio"
              aria-checked={selected}
              data-testid={`skin-${s.id}`}
              className={`skin-option${selected ? " on" : ""}`}
              onClick={() => setSkin(s.id)}
            >
              <span className="skin-preview" aria-hidden="true">
                <SkinPreview skin={s} resolveCustom={resolveCustom} />
              </span>
              <span className="skin-meta">
                <span className="skin-name">{s.name}</span>
                <span className="skin-desc">{s.desc}</span>
              </span>
            </button>
          );
        })}
      </div>
      <p className="skin-note">当前形象：{skin.name}。形象只是外观，阿盆的记忆和成长不受影响。</p>
    </div>
  );
}

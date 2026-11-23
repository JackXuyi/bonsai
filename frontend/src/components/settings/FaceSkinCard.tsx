"use client";

// 设置页 · 形象卡（plan-face-custom-skins.md §6.2）：
// 1. 皮肤列表单选（内置 2 + 自定义 N）；
// 2. 选中自定义皮肤 → 7 槽位网格（上传/清除）+ 删除；
// 3. 新建皮肤输入行 + 说明文案（上限 + 隐私声明）。
import { useEffect, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import {
  BUILTIN_SKINS,
  DEFAULT_SKIN,
  SkinPreview,
  faceSkinById,
  type FaceSkin,
} from "@/components/face/skins";
import { SkinSlotGrid } from "./SkinSlotGrid";
import { ensureFaceSkinsLoaded, useFaceSkinStore } from "@/lib/stores/faceSkinStore";
import {
  faceSkinClearSlot,
  faceSkinCreate,
  faceSkinDelete,
  faceSkinPickAndImport,
} from "@/lib/commands";
import { useUiStore } from "@/lib/stores/uiStore";

/** Rust 结构化错误 `face_<reason>: 中文说明` → 只取说明。 */
function errText(e: unknown): string {
  const raw = String(e);
  const idx = raw.indexOf(":");
  return idx > 0 ? raw.slice(idx + 1) : raw;
}

export function FaceSkinCard() {
  const skinId = useFaceSkinStore((s) => s.skinId);
  const customSkins = useFaceSkinStore((s) => s.customSkins);
  const baseDir = useFaceSkinStore((s) => s.baseDir);
  const setSkin = useFaceSkinStore((s) => s.setSkin);
  const refreshCustomSkins = useFaceSkinStore((s) => s.refreshCustomSkins);
  const [newName, setNewName] = useState("");
  const [busy, setBusy] = useState(false);

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

  async function handleCreate() {
    if (!newName.trim() || busy) return;
    setBusy(true);
    try {
      const m = await faceSkinCreate(newName.trim());
      setNewName("");
      await refreshCustomSkins();
      setSkin(m.id);
      useUiStore.getState().pushToast(`已创建「${m.name}」，先上传 calm 图`);
    } catch (e) {
      useUiStore.getState().pushToast(errText(e));
    } finally {
      setBusy(false);
    }
  }

  async function handleUpload(slot: string) {
    if (!skin.custom || busy) return;
    setBusy(true);
    try {
      await faceSkinPickAndImport(skin.id, slot);
      await refreshCustomSkins();
    } catch (e) {
      // 取消选择不弹错
      if (!String(e).includes("face_pick_cancelled")) {
        useUiStore.getState().pushToast(errText(e));
      }
    } finally {
      setBusy(false);
    }
  }

  async function handleClear(slot: string) {
    if (!skin.custom || busy) return;
    setBusy(true);
    try {
      await faceSkinClearSlot(skin.id, slot);
      await refreshCustomSkins();
    } catch (e) {
      useUiStore.getState().pushToast(errText(e));
    } finally {
      setBusy(false);
    }
  }

  async function handleDelete() {
    if (!skin.custom || busy) return;
    if (!window.confirm(`删除皮肤「${skin.name}」？图片会一并清掉。`)) return;
    setBusy(true);
    try {
      await faceSkinDelete(skin.id);
      await refreshCustomSkins();
      setSkin(DEFAULT_SKIN);
    } catch (e) {
      useUiStore.getState().pushToast(errText(e));
    } finally {
      setBusy(false);
    }
  }

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

      {/* 选中自定义皮肤 → 槽位管理（用 kind 判别收窄，manifest 只在 custom 分支） */}
      {skin.kind === "custom" && (
        <div className="skin-manage" data-testid="skin-manage">
          <div className="skin-manage-head">
            <span className="skin-manage-title">「{skin.name}」的表情图</span>
            <button type="button" className="skin-danger" data-testid="skin-delete" onClick={handleDelete}>
              删除此皮肤
            </button>
          </div>
          <SkinSlotGrid
            skinId={skin.id}
            slots={skin.manifest.slots}
            resolveSrc={resolveCustom}
            onUpload={handleUpload}
            onClear={handleClear}
          />
          <p className="skin-slot-hint">
            calm 是兜底表情（<b>必传</b>）；其余表情缺图时回退 calm。每个表情对应一张图，建议 200×200 以上、方形、透明背景。
          </p>
        </div>
      )}

      {/* 新建皮肤 */}
      <div className="skin-create-row" data-testid="skin-create">
        <input
          className="skin-create-input"
          data-testid="skin-create-input"
          placeholder="新皮肤名称（如：我的猫）"
          maxLength={20}
          value={newName}
          onChange={(e) => setNewName(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && handleCreate()}
        />
        <button
          type="button"
          className="skin-create-btn"
          data-testid="skin-create-btn"
          disabled={busy || !newName.trim()}
          onClick={handleCreate}
        >
          新建皮肤
        </button>
      </div>

      <p className="skin-note">
        当前形象：{skin.name}。图片只保存在本机（≤2MB/张、≤10 套），不会上传到任何地方；形象只是外观，阿盆的记忆和成长不受影响。
      </p>
    </div>
  );
}

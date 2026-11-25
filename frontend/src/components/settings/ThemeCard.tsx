"use client";

// 设置 · 外观主题色（plan-presence-im v2/v4）。
import { useEffect, useState } from "react";
import { THEMES, applyTheme, readStoredThemeId, type ThemeId } from "@/lib/theme";

export function ThemeCard() {
  const [id, setId] = useState<ThemeId>(DEFAULT_SAFE);

  useEffect(() => {
    const cur = readStoredThemeId();
    setId(cur);
    applyTheme(cur);
  }, []);

  function pick(next: ThemeId) {
    setId(next);
    applyTheme(next);
  }

  return (
    <div className="card theme-card" data-testid="theme-card">
      <h3 className="card-title">外观</h3>
      <p className="hint-line">主题色即时切换，只改界面变量，不改表情素材。</p>
      <div className="theme-options" role="radiogroup" aria-label="主题色">
        {THEMES.map((t) => (
          <button
            key={t.id}
            type="button"
            role="radio"
            aria-checked={id === t.id}
            data-testid={`theme-${t.id}`}
            className={`theme-option${id === t.id ? " on" : ""}`}
            onClick={() => pick(t.id)}
          >
            <span className="theme-swatch" style={{ background: t.swatch }} aria-hidden="true" />
            <span className="theme-label">{t.label}</span>
          </button>
        ))}
      </div>
    </div>
  );
}

function DEFAULT_SAFE(): ThemeId {
  return readStoredThemeId();
}

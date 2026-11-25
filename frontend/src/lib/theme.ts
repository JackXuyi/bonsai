// 主题色包（plan-presence-im v2/v4）。只改 CSS 变量，业务组件继续吃变量。

export type ThemeId = "whale" | "moss" | "ink" | "clay" | "paper";

export interface ThemeDef {
  id: ThemeId;
  label: string;
  /** 设置色板预览用 */
  swatch: string;
  vars: Record<string, string>;
}

export const DEFAULT_THEME: ThemeId = "whale";

export const THEMES: ThemeDef[] = [
  {
    id: "whale",
    label: "鲸蓝",
    swatch: "#4d6bfe",
    vars: {
      "--bg": "#eef1f8",
      "--bg-sunk": "#dfe5f2",
      "--card": "#f8f9fd",
      "--ink": "#1e2433",
      "--muted": "#6b7388",
      "--faint": "#9aa3b8",
      "--accent": "#4d6bfe",
      "--accent-ink": "#3a52d4",
      "--accent-wash": "rgba(77, 107, 254, 0.12)",
      "--line": "#d5dceb",
      "--line-soft": "rgba(30, 36, 51, 0.07)",
      "--shadow": "0 2px 8px rgba(30, 36, 51, 0.08)",
      "--ok": "#4d6bfe",
      "--warn": "#b07d22",
      "--danger": "#b5533c",
      "--leaf": "#4d6bfe",
      "--leaf-dark": "#3a52d4",
      "--leaf-light": "#7b91ff",
    },
  },
  {
    id: "moss",
    label: "苔绿",
    swatch: "#4c8c2b",
    vars: {
      "--bg": "#f4f1e9",
      "--bg-sunk": "#e7e1d1",
      "--card": "#fffdf8",
      "--ink": "#322f20",
      "--muted": "#8a8468",
      "--faint": "#a9a28a",
      "--accent": "#4c8c2b",
      "--accent-ink": "#3f7024",
      "--accent-wash": "rgba(76, 140, 43, 0.1)",
      "--line": "#e3ddcb",
      "--line-soft": "rgba(60, 50, 30, 0.07)",
      "--shadow": "0 2px 8px rgba(60, 50, 30, 0.07)",
      "--ok": "#4c8c2b",
      "--warn": "#b07d22",
      "--danger": "#b5533c",
      "--leaf": "#4c8c2b",
      "--leaf-dark": "#3a6e21",
      "--leaf-light": "#6aa846",
    },
  },
  {
    id: "ink",
    label: "墨青",
    swatch: "#3d5a80",
    vars: {
      "--bg": "#1a1f2a",
      "--bg-sunk": "#12161f",
      "--card": "#232a38",
      "--ink": "#e8ecf4",
      "--muted": "#9aa3b5",
      "--faint": "#6f788c",
      "--accent": "#6b8cae",
      "--accent-ink": "#9bb4d0",
      "--accent-wash": "rgba(107, 140, 174, 0.18)",
      "--line": "#343c4e",
      "--line-soft": "rgba(232, 236, 244, 0.08)",
      "--shadow": "0 2px 10px rgba(0, 0, 0, 0.35)",
      "--ok": "#6b8cae",
      "--warn": "#c9a227",
      "--danger": "#d47060",
      "--leaf": "#6b8cae",
      "--leaf-dark": "#4a6a8a",
      "--leaf-light": "#8eabca",
    },
  },
  {
    id: "clay",
    label: "陶土",
    swatch: "#a66f38",
    vars: {
      "--bg": "#f3ebe3",
      "--bg-sunk": "#e6d8cb",
      "--card": "#fbf6f0",
      "--ink": "#3a2a1f",
      "--muted": "#8a7360",
      "--faint": "#b09a86",
      "--accent": "#a66f38",
      "--accent-ink": "#8b5a2b",
      "--accent-wash": "rgba(166, 111, 56, 0.12)",
      "--line": "#e0d2c3",
      "--line-soft": "rgba(58, 42, 31, 0.07)",
      "--shadow": "0 2px 8px rgba(58, 42, 31, 0.08)",
      "--ok": "#6a8f4e",
      "--warn": "#b07d22",
      "--danger": "#b5533c",
      "--leaf": "#a66f38",
      "--leaf-dark": "#8b5a2b",
      "--leaf-light": "#c48955",
    },
  },
  {
    id: "paper",
    label: "素纸",
    swatch: "#333333",
    vars: {
      "--bg": "#f7f7f5",
      "--bg-sunk": "#ecece8",
      "--card": "#ffffff",
      "--ink": "#1a1a1a",
      "--muted": "#6e6e6e",
      "--faint": "#9a9a9a",
      "--accent": "#333333",
      "--accent-ink": "#111111",
      "--accent-wash": "rgba(0, 0, 0, 0.06)",
      "--line": "#e2e2de",
      "--line-soft": "rgba(0, 0, 0, 0.05)",
      "--shadow": "0 1px 4px rgba(0, 0, 0, 0.06)",
      "--ok": "#2f6f4e",
      "--warn": "#a67c1a",
      "--danger": "#a94442",
      "--leaf": "#333333",
      "--leaf-dark": "#111111",
      "--leaf-light": "#666666",
    },
  },
];

const STORAGE_KEY = "bonsai-theme";

export function themeById(id: string | null | undefined): ThemeDef {
  return THEMES.find((t) => t.id === id) ?? THEMES.find((t) => t.id === DEFAULT_THEME)!;
}

export function readStoredThemeId(): ThemeId {
  if (typeof window === "undefined") return DEFAULT_THEME;
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (raw && THEMES.some((t) => t.id === raw)) return raw as ThemeId;
  } catch {
    /* ignore */
  }
  return DEFAULT_THEME;
}

export function applyTheme(id: ThemeId) {
  if (typeof document === "undefined") return;
  const theme = themeById(id);
  const root = document.documentElement;
  root.setAttribute("data-theme", theme.id);
  for (const [k, v] of Object.entries(theme.vars)) {
    root.style.setProperty(k, v);
  }
  try {
    localStorage.setItem(STORAGE_KEY, theme.id);
  } catch {
    /* ignore */
  }
}

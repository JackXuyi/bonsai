import type { Config } from "tailwindcss";

// M0a：样式沿用迁移的语义 CSS（见 src/app/*.css，数值逐项对齐旧版）。
// 这里只把 :root 变量接进 tailwind 色板，供 M1+ 新面板（记忆/设置页）使用。
// preflight 关闭：迁移期组件样式完全由移植的语义 CSS 控制，避免 reset 覆盖。
const config: Config = {
  content: ["./src/**/*.{ts,tsx}"],
  corePlugins: { preflight: false },
  theme: {
    extend: {
      colors: {
        bg: "var(--bg)",
        "bg-sunk": "var(--bg-sunk)",
        card: "var(--card)",
        pot: "var(--pot)",
        "pot-rim": "var(--pot-rim)",
        trunk: "var(--trunk)",
        leaf: "var(--leaf)",
        "leaf-dark": "var(--leaf-dark)",
        "leaf-light": "var(--leaf-light)",
        ink: "var(--ink)",
        muted: "var(--muted)",
        faint: "var(--faint)",
        accent: "var(--accent)",
        "accent-ink": "var(--accent-ink)",
        "accent-wash": "var(--accent-wash)",
        line: "var(--line)",
        "line-soft": "var(--line-soft)",
      },
    },
  },
  plugins: [],
};

export default config;

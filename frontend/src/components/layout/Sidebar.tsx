"use client";

// 左侧栏（spec-m0b §1）：品牌区（迷你脸 + 名字 + 情绪点行，caption 取消）
// + 5 个导航项（usePathname 高亮）。宽度 92px，全高不滚。
import Link from "next/link";
import { usePathname } from "next/navigation";
import { PokeFace } from "@/components/face/PokeFace";
import { MOODS, MOOD_COLORS } from "@/components/face/frames";
import { useFaceFrame } from "@/hooks/useFaceFrame";
import { normalizePath, ROUTES } from "@/lib/shortcut";
import { useUiStore } from "@/lib/stores/uiStore";

const ICONS: Record<string, React.ReactNode> = {
  "/": (
    <svg viewBox="0 0 16 16" aria-hidden="true">
      <path d="M3 4.5h10M3 8h7M3 11.5h5" />
    </svg>
  ),
  "/face": (
    <svg viewBox="0 0 16 16" aria-hidden="true">
      <circle cx="8" cy="8" r="5.5" />
      <path d="M6 7h.01M10 7h.01M6.2 10q1.8 1.4 3.6 0" />
    </svg>
  ),
  "/tools": (
    <svg viewBox="0 0 16 16" aria-hidden="true">
      <rect x="3" y="3" width="4.4" height="4.4" rx="1" />
      <rect x="8.6" y="3" width="4.4" height="4.4" rx="1" />
      <rect x="3" y="8.6" width="4.4" height="4.4" rx="1" />
      <rect x="8.6" y="8.6" width="4.4" height="4.4" rx="1" />
    </svg>
  ),
  "/memory": (
    <svg viewBox="0 0 16 16" aria-hidden="true">
      <ellipse cx="8" cy="4.4" rx="5" ry="2" />
      <path d="M3 4.4v7c0 1.1 2.2 2 5 2s5-.9 5-2v-7" />
    </svg>
  ),
  "/settings": (
    <svg viewBox="0 0 16 16" aria-hidden="true">
      <circle cx="8" cy="8" r="2.4" />
      <path d="M8 1.8v2M8 12.2v2M1.8 8h2M12.2 8h2M3.6 3.6l1.4 1.4M11 11l1.4 1.4M12.4 3.6L11 5M5 11l-1.4 1.4" />
    </svg>
  ),
};

export function Sidebar() {
  const pathname = normalizePath(usePathname());
  const faceState = useUiStore((s) => s.faceState);
  const frame = useFaceFrame();

  return (
    <aside className="sidebar">
      <div className="side-brand">
        <PokeFace size="sm" frame={frame} />
        <p className="side-name">阿盆</p>
        <p className="side-mood">
          <span
            className="mood-dot"
            style={{ background: MOOD_COLORS[faceState] ?? MOOD_COLORS.calm }}
            aria-hidden="true"
          />
          {MOODS[faceState] ?? MOODS.calm}
        </p>
      </div>

      <nav className="side-nav" aria-label="面板">
        {ROUTES.map((r) => {
          const active = pathname === r.path;
          return (
            <Link
              key={r.path}
              href={r.path}
              className={`side-item${active ? " active" : ""}`}
              aria-current={active ? "page" : undefined}
            >
              <span className="side-icon">{ICONS[r.path]}</span>
              <span className="side-label">{r.label}</span>
            </Link>
          );
        })}
      </nav>
    </aside>
  );
}

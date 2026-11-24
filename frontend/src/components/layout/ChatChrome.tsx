"use client";

// Companion 顶栏三槽：≡ 会话 · 存在感 · ⚙ 设置（plan-chat-first-ui v2）。
import Link from "next/link";
import { PresenceBadge } from "@/components/layout/PresenceBadge";
import { useUiStore } from "@/lib/stores/uiStore";

export function ChatChrome() {
  const open = useUiStore((s) => s.sessionDrawerOpen);
  const setOpen = useUiStore((s) => s.setSessionDrawerOpen);

  return (
    <header className="chat-chrome">
      <button
        type="button"
        className="chrome-icon-btn"
        aria-label="会话"
        aria-expanded={open}
        title="会话（⌘⇧O）"
        onClick={() => setOpen(!open)}
      >
        <svg viewBox="0 0 24 24" aria-hidden="true">
          <line x1="4" y1="7" x2="20" y2="7" />
          <line x1="4" y1="12" x2="13" y2="12" />
          <line x1="4" y1="17" x2="16" y2="17" />
        </svg>
      </button>

      <PresenceBadge />

      <Link
        href="/settings"
        className="chrome-icon-btn"
        aria-label="设置"
        title="设置（⌘,）"
      >
        <svg viewBox="0 0 24 24" aria-hidden="true">
          <circle cx="12" cy="12" r="3" />
          <path d="M12 2v2M12 20v2M2 12h2M20 12h2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M19.1 4.9l-1.4 1.4M6.3 17.7l-1.4 1.4" />
        </svg>
      </Link>
    </header>
  );
}

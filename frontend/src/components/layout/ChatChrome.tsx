"use client";

// Companion 顶栏：左 ≡ 会话；右 ⚙ 设置 + 头像（头像最右）。
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

      <div className="chat-chrome-end">
        <Link
          href="/settings"
          className="chrome-icon-btn"
          aria-label="设置"
          title="设置（⌘,）"
        >
          {/* 经典齿轮（cog），对标系统 settings / gearshape */}
          <svg viewBox="0 0 24 24" aria-hidden="true">
            <circle cx="12" cy="12" r="3" />
            <path d="M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z" />
          </svg>
        </Link>
        <PresenceBadge />
      </div>
    </header>
  );
}

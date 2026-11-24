"use client";

// 二级页顶栏：← + 标题（设置索引/详情、trace）。
import Link from "next/link";

export interface SecondaryChromeProps {
  title: string;
  /** 返回目标；默认对话。 */
  backHref?: string;
  backLabel?: string;
}

export function SecondaryChrome({
  title,
  backHref = "/",
  backLabel = "返回对话",
}: SecondaryChromeProps) {
  return (
    <header className="secondary-chrome">
      <Link href={backHref} className="chrome-back" aria-label={backLabel}>
        <svg viewBox="0 0 24 24" aria-hidden="true">
          <polyline points="15 6 9 12 15 18" />
        </svg>
        <span>{backLabel}</span>
      </Link>
      <h1 className="secondary-title">{title}</h1>
      <span className="secondary-chrome-spacer" aria-hidden="true" />
    </header>
  );
}

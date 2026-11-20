// 展示用格式化（模型卡 / 下载行共用）。

/** 800_000_000 → "0.8GB"；不足 100MB 用 MB。 */
export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "—";
  const gb = bytes / 1_000_000_000;
  if (gb >= 0.1) return `${gb.toFixed(1).replace(/\.0$/, "")}GB`;
  return `${Math.round(bytes / 1_000_000)}MB`;
}

/** ISO 时间 → "3 天前" / "今天"（模型卡副行用；坏值回落空串）。 */
export function relativeDay(iso: string, now: Date = new Date()): string {
  const t = Date.parse(iso);
  if (Number.isNaN(t)) return "";
  const days = Math.floor((now.getTime() - t) / 86_400_000);
  if (days <= 0) return "今天";
  if (days === 1) return "昨天";
  if (days < 30) return `${days} 天前`;
  return `${Math.floor(days / 30)} 个月前`;
}

/**
 * ISO 时间 → "今天 14:02" / "昨天 19:40" / "3 天前 09:12"。
 * 后端只发 RFC3339 原值，本地化在这里做（spec-m3 §1）。
 */
export function formatWhen(iso: string, now: Date = new Date()): string {
  const t = Date.parse(iso);
  if (Number.isNaN(t)) return "";
  const d = new Date(t);
  const hh = String(d.getHours()).padStart(2, "0");
  const mm = String(d.getMinutes()).padStart(2, "0");
  const day = relativeDay(iso, now);
  return day ? `${day} ${hh}:${mm}` : `${hh}:${mm}`;
}

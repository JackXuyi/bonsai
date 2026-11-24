// 快捷键（plan-chat-first-ui v2）：废弃 ⌘1–⌘5 功能页；
// ⌘, → 设置；⌘⇧O → 会话抽屉（由 hook 翻 uiStore，不走路由）。
// 纯函数部分可单测；抽屉开关见 useAppShortcuts。

export interface RouteDef {
  path: string;
  label: string;
}

/** 仍用于 titleForPath 等标题映射；不再驱动侧栏。 */
export const ROUTES: RouteDef[] = [
  { path: "/", label: "对话" },
  { path: "/settings", label: "设置" },
  { path: "/face", label: "形象" },
  { path: "/tools", label: "权限与工具" },
  { path: "/memory", label: "记忆" },
];

export type ShortcutAction =
  | { kind: "route"; path: string }
  | { kind: "toggleSessions" };

/**
 * meta/ctrl + 键 → 动作。
 * - `,` → 设置
 * - `O` + shift → 会话抽屉
 * 其它返回 null。
 */
export function shortcutAction(
  meta: boolean,
  shift: boolean,
  key: string
): ShortcutAction | null {
  if (!meta) return null;
  if (key === "," || key === "，") return { kind: "route", path: "/settings" };
  if (shift && (key === "o" || key === "O")) return { kind: "toggleSessions" };
  return null;
}

/** @deprecated 用 shortcutAction；保留给旧测迁移期兼容检查。 */
export function routeForKeys(meta: boolean, key: string): string | null {
  const a = shortcutAction(meta, false, key);
  return a?.kind === "route" ? a.path : null;
}

/** 路由 → 页头标题（未知名路由回落「阿盆」）。 */
export function titleForPath(path: string): string {
  if (path.startsWith("/trace")) return "生成链路";
  if (path.startsWith("/settings")) return "设置";
  return ROUTES.find((r) => r.path === path)?.label ?? "阿盆";
}

/**
 * 归一化 usePathname 的结果：静态导出直开 /xxx.html 时 pathname 会带
 * .html 后缀（导致高亮失效 + 水合不匹配），剥掉并对齐 index.html → /。
 */
export function normalizePath(p: string): string {
  if (p === "/index.html") return "/";
  const noHtml = p.replace(/\.html$/, "");
  return noHtml === "" ? "/" : noHtml;
}

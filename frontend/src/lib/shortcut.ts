// 快捷键路由映射（spec-m0b §1）：⌘1–⌘5 → 五条路由。
// 纯函数，无 React 依赖，单测直接覆盖。

export interface RouteDef {
  path: string;
  label: string;
}

/** 侧栏五项（顺序 = ⌘1–⌘5）。对话固定最前。 */
export const ROUTES: RouteDef[] = [
  { path: "/", label: "对话" },
  { path: "/face", label: "表情" },
  { path: "/tools", label: "工具" },
  { path: "/memory", label: "记忆" },
  { path: "/settings", label: "设置" },
];

const ROUTE_BY_KEY = new Map(
  ROUTES.map((r, i) => [String(i + 1), r] as const)
);

/** metaKey/ctrlKey + 数字键 → 路由；不匹配返回 null。 */
export function routeForKeys(meta: boolean, key: string): string | null {
  if (!meta) return null;
  return ROUTE_BY_KEY.get(key)?.path ?? null;
}

/** 路由 → 页头标题（未知名路由回落「阿盆」）。 */
export function titleForPath(path: string): string {
  return ROUTES.find((r) => r.path === path)?.label ?? "阿盆";
}

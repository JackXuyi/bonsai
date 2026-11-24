// 设置分区元数据（plan-chat-first-ui v2）：索引行 + 旧路由深链映射。
// 纯函数，无 React 依赖，单测直接覆盖。

export type SettingsSectionId =
  | "presence"
  | "persona"
  | "memory"
  | "model"
  | "agent"
  | "access"
  | "ops";

export interface SettingsSectionDef {
  id: SettingsSectionId;
  label: string;
  /** 索引页分组标题 */
  group: string;
}

/** 索引顺序 = 展示顺序（组内按数组序）。 */
export const SETTINGS_SECTIONS: SettingsSectionDef[] = [
  { id: "presence", label: "形象", group: "我与它" },
  { id: "persona", label: "人格与关系", group: "我与它" },
  { id: "memory", label: "记忆", group: "我与它" },
  { id: "model", label: "模型", group: "智能" },
  { id: "agent", label: "能力", group: "智能" },
  { id: "access", label: "权限与工具", group: "系统" },
  { id: "ops", label: "运维", group: "系统" },
];

const BY_ID = new Map(SETTINGS_SECTIONS.map((s) => [s.id, s] as const));

/** 旧平级路由 → 设置分区（redirect 用）。 */
export const LEGACY_PATH_TO_SECTION: Record<string, SettingsSectionId> = {
  "/face": "presence",
  "/tools": "access",
  "/memory": "memory",
};

export function sectionById(id: string | null | undefined): SettingsSectionDef | null {
  if (!id) return null;
  return BY_ID.get(id as SettingsSectionId) ?? null;
}

export function settingsHref(section?: SettingsSectionId): string {
  return section ? `/settings?section=${section}` : "/settings";
}

/** 按 group 聚合，保 SETTINGS_SECTIONS 顺序。 */
export function settingsGroups(): { group: string; items: SettingsSectionDef[] }[] {
  const out: { group: string; items: SettingsSectionDef[] }[] = [];
  for (const s of SETTINGS_SECTIONS) {
    const last = out[out.length - 1];
    if (last && last.group === s.group) last.items.push(s);
    else out.push({ group: s.group, items: [s] });
  }
  return out;
}

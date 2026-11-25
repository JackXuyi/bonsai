/** 工具参数展示：优先按行拆成键值；坏格式则原样。 */

export interface ToolArgLine {
  key?: string;
  value: string;
}

/** 把后端 argsSummary / argsJson 拆成可读行（已是「键：值」分行或 JSON）。 */
export function parseToolArgLines(raw: string): ToolArgLine[] {
  const text = raw.trim();
  if (!text) return [];

  // 后端 format_args_summary 已是「键：值」多行
  if (text.includes("：") && !text.trimStart().startsWith("{")) {
    return text.split("\n").map((line) => {
      const i = line.indexOf("：");
      if (i <= 0) return { value: line };
      return { key: line.slice(0, i), value: line.slice(i + 1) };
    });
  }

  try {
    const j = JSON.parse(text) as unknown;
    if (j && typeof j === "object" && !Array.isArray(j)) {
      const obj = j as Record<string, unknown>;
      const keys = Object.keys(obj).sort();
      if (keys.length === 0) return [{ value: "（无参数）" }];
      return keys.map((k) => ({
        key: k,
        value: typeof obj[k] === "string" ? (obj[k] as string) : JSON.stringify(obj[k], null, 2),
      }));
    }
    return [{ value: JSON.stringify(j, null, 2) }];
  } catch {
    return [{ value: text }];
  }
}

/** 折叠预览（一行）。 */
export function toolArgsPreview(raw: string, max = 120): string {
  const one = raw.replace(/\s+/g, " ").trim();
  return one.length <= max ? one : `${one.slice(0, max)}…`;
}

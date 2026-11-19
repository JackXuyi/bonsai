/**
 * composer 的 Enter 提交判定（从 ui/main.js 的 keydown 逻辑抽出）。
 * @returns true = 阻止提交（换行或 IME 组合中）
 */
export function shouldBlockSubmit(
  isComposing: boolean,
  key: string,
  shiftKey: boolean
): boolean {
  if (key !== "Enter") return true;
  if (shiftKey) return true;
  return isComposing;
}

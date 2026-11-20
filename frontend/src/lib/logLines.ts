// 日志行分级（spec-m4 §4B）：纯函数，着色用。
// simplelog 默认行形如 `21:05:51 [WARN bonsai] ...` / `[ERROR ...]`，
// sidecar stderr 是 ollama 自己的输出，也含大写级别词。
export type LogLevel = "warn" | "error" | "info" | "plain";

export function classifyLine(line: string): LogLevel {
  if (/\bERROR\b/.test(line)) return "error";
  if (/\bWARN\b/.test(line)) return "warn";
  if (/\bINFO\b/.test(line)) return "info";
  return "plain";
}

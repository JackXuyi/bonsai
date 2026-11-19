import { invoke } from "@tauri-apps/api/core";

const DEFAULT_TIMEOUT_MS = 15000;

export const inTauri =
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

/**
 * invoke 带 15s 超时兜底（meetily 同款 Promise.race）。
 * 防 IPC 挂死白屏：命令不响应时抛错而不是永远 pending。
 */
export function invokeWithTimeout<T>(
  cmd: string,
  args?: Record<string, unknown>,
  timeoutMs: number = DEFAULT_TIMEOUT_MS
): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const timeout = new Promise<never>((_, reject) => {
    timer = setTimeout(
      () => reject(new Error(`[IPC] ${cmd} did not respond within ${timeoutMs}ms`)),
      timeoutMs
    );
  });
  const call = invoke<T>(cmd, args).finally(() => clearTimeout(timer));
  return Promise.race([call, timeout]);
}

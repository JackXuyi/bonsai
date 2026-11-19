"use client";

import { useEffect, useRef } from "react";
import { listen as tauriListen, type UnlistenFn } from "@tauri-apps/api/event";
import { inTauri } from "./tauriInvoke";

/**
 * 事件订阅唯一出口。浏览器预览（无 Tauri runtime）时返回 no-op。
 * handler 收到的直接是 event.payload。
 */
export async function listen<T>(
  event: string,
  handler: (payload: T) => void
): Promise<UnlistenFn> {
  if (!inTauri) return () => {};
  try {
    return await tauriListen<T>(event, (e) => handler(e.payload));
  } catch {
    return () => {};
  }
}

/** 幂等卸载：unlisten 可能因竞态为 null，或被调用多次。 */
export function safeUnlisten(fn?: UnlistenFn | null) {
  if (!fn) return;
  try {
    fn();
  } catch {
    /* already unlistened — fine */
  }
}

/**
 * 组件级订阅：useEffect 内注册、cleanup 卸载。
 * handler 用 ref 持有，handler 闭包里的 state 永远是最新值，不重复订阅。
 */
export function useTauriEvent<T>(event: string, handler: (payload: T) => void) {
  const ref = useRef(handler);
  ref.current = handler;
  useEffect(() => {
    let un: UnlistenFn | null = null;
    let disposed = false;
    listen<T>(event, (p) => ref.current(p)).then((u) => {
      if (disposed) {
        u();
      } else {
        un = u;
      }
    });
    return () => {
      disposed = true;
      safeUnlisten(un);
    };
  }, [event]);
}

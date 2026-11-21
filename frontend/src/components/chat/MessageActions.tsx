"use client";

// 气泡底部操作条（spec-s8 S8-c）：时间常驻灰字，复制 / 查看详情 hover 浮现。
// 「查看详情」跳 /trace/[turnId]；历史气泡只带 eventId 时点击反查（旧消息
// 查不到 → toast 说明，而不是预知性的隐藏入口）。
import { useCallback } from "react";
import { useRouter } from "next/navigation";
import { turnTraceFindByEvent } from "@/lib/commands";
import { inTauri } from "@/lib/tauriInvoke";
import { formatWhen } from "@/lib/format";
import { toast } from "@/lib/stores/uiStore";
import type { ChatMessage } from "@/lib/stores/chatStore";

export function MessageActions({ message }: { message: ChatMessage }) {
  const router = useRouter();
  const canTrace = Boolean(message.turnId || message.eventId);

  const copy = useCallback(() => {
    const ok = () => toast("已复制");
    const fail = () => toast("复制失败");
    if (navigator.clipboard) {
      navigator.clipboard.writeText(message.text).then(ok, fail);
    } else {
      fail();
    }
  }, [message.text]);

  const openTrace = useCallback(() => {
    if (message.turnId) {
      // 静态导出（output: export）不支持动态路由段，trace 入口统一走 query
      router.push(`/trace?turn=${message.turnId}`);
      return;
    }
    if (!inTauri || !message.eventId) return;
    void turnTraceFindByEvent(message.eventId)
      .then((tid) => {
        if (tid) router.push(`/trace?turn=${tid}`);
        else toast("这条消息产生于链路追踪上线前，没有留下记录");
      })
      .catch(() => toast("查不到追踪记录"));
  }, [message.turnId, message.eventId, router]);

  return (
    <div className={`msg-actions${message.kind === "user" ? " user" : ""}`}>
      {message.ts && <span className="msg-time">{formatWhen(message.ts)}</span>}
      <button type="button" className="msg-act" onClick={copy}>
        复制
      </button>
      {canTrace && (
        <button type="button" className="msg-act trace" onClick={openTrace}>
          查看详情 →
        </button>
      )}
    </div>
  );
}

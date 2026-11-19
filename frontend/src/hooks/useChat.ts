"use client";

// 发送 + 流式接收（旧 sendMessage：Channel onDelta 渐增气泡，typing 占位
// 首块替换；无 delta 时整段渲染；错误撤气泡）。
import { useCallback, useRef, useState } from "react";
import { Channel } from "@tauri-apps/api/core";
import { dialogueChatStream, facePreview } from "@/lib/commands";
import { inTauri } from "@/lib/tauriInvoke";
import { noteFor } from "@/components/face/frames";
import { useChatStore } from "@/lib/stores/chatStore";
import { toast } from "@/lib/stores/uiStore";

const SESSION_ID = "default";

export function useChat() {
  const [chatting, setChatting] = useState(false);
  const chattingRef = useRef(false);
  const chat = useChatStore;

  const send = useCallback(async (raw: string) => {
    const text = raw.trim();
    if (!text || chattingRef.current) return;

    const store = chat.getState();
    if (!inTauri) {
      store.add({ kind: "user", text });
      store.add({
        kind: "bot",
        text: "（浏览器预览模式：Rust 后端未连接，发消息不会有人回。）",
      });
      return;
    }

    store.add({ kind: "user", text });
    chattingRef.current = true;
    setChatting(true);
    const typingId = store.add({ kind: "typing", text: "" });

    // Streaming: deltas arrive while the model is still generating; the
    // typing placeholder is swapped for a real bubble on the first chunk.
    let bubbleId: number | null = null;
    const onDelta = new Channel<{ delta?: string }>();
    onDelta.onmessage = (msg) => {
      if (!msg?.delta) return;
      if (bubbleId === null) {
        store.remove(typingId);
        bubbleId = store.add({ kind: "bot", text: "", streaming: true });
      }
      store.appendText(bubbleId, msg.delta);
    };

    try {
      const outcome = await dialogueChatStream(text, SESSION_ID, onDelta);
      if (bubbleId === null) {
        // No deltas at all (fallback line / dead model) — render whole reply.
        store.remove(typingId);
        bubbleId = store.add({ kind: "bot", text: "" });
      }
      store.setText(bubbleId, outcome.reply);
      store.setNote(bubbleId, noteFor(outcome));
      // Speaking counts as "being talked to": flash the greeted face.
      facePreview("greeted").catch(() => {});
    } catch (err) {
      if (bubbleId !== null) store.remove(bubbleId);
      store.remove(typingId);
      store.add({ kind: "bot", text: `（出错了：${err}）` });
      toast("没发出去，再看看日志");
    } finally {
      chattingRef.current = false;
      setChatting(false);
    }
  }, [chat]);

  return { send, chatting };
}

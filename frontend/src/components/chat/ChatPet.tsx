"use client";

// 聊天框右上角互动头像（plan-presence-im v2/v4）。
// 有消息时常驻；空态隐藏，避免与居中大脸双脸。
import { useEffect } from "react";
import { PokeFace } from "@/components/face/PokeFace";
import { useFaceFrame } from "@/hooks/useFaceFrame";
import { usePersonaStore } from "@/lib/stores/personaStore";
import { useChatStore } from "@/lib/stores/chatStore";
import type { FaceState } from "@/lib/types";

export function ChatPet({ chatting }: { chatting: boolean }) {
  const messages = useChatStore((s) => s.messages);
  const frame = useFaceFrame();
  const ensurePersona = usePersonaStore((s) => s.ensureLoaded);

  useEffect(() => {
    ensurePersona();
  }, [ensurePersona]);

  if (messages.length === 0) return null;

  const override: FaceState | undefined = chatting ? "thinking" : undefined;

  return (
    <div className="chat-pet" data-testid="chat-pet">
      <PokeFace size="md" frame={frame} stateOverride={override} />
    </div>
  );
}

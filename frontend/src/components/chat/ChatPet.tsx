"use client";

// 输入框右上角互动头像（plan-presence-im）：贴在 composer 上沿，
// 默认「偷看」表情，有在注视你的感觉；流式中切 thinking。
import { useEffect } from "react";
import { PokeFace } from "@/components/face/PokeFace";
import { useFaceFrame } from "@/hooks/useFaceFrame";
import { usePersonaStore } from "@/lib/stores/personaStore";
import type { FaceState } from "@/lib/types";

export function ChatPet({ chatting }: { chatting: boolean }) {
  const frame = useFaceFrame();
  const ensurePersona = usePersonaStore((s) => s.ensureLoaded);

  useEffect(() => {
    ensurePersona();
  }, [ensurePersona]);

  const override: FaceState | undefined = chatting ? "thinking" : undefined;

  return (
    <div className="chat-pet" data-testid="chat-pet" aria-hidden={false}>
      <PokeFace size="md" frame={frame} stateOverride={override} />
    </div>
  );
}

"use client";

// 存在感：短名 + 情绪点 + 迷你脸（脸在最右，贴顶栏右缘）。
import { PokeFace } from "@/components/face/PokeFace";
import { MOODS, MOOD_COLORS } from "@/components/face/frames";
import { useFaceFrame } from "@/hooks/useFaceFrame";
import { useUiStore } from "@/lib/stores/uiStore";

export function PresenceBadge() {
  const faceState = useUiStore((s) => s.faceState);
  const frame = useFaceFrame();

  return (
    <div className="presence-badge">
      <div className="presence-meta">
        <span className="presence-name">阿盆</span>
        <span className="presence-mood">
          <span
            className="mood-dot"
            style={{ background: MOOD_COLORS[faceState] ?? MOOD_COLORS.calm }}
            aria-hidden="true"
          />
          {MOODS[faceState] ?? MOODS.calm}
        </span>
      </div>
      <PokeFace size="xs" frame={frame} />
    </div>
  );
}

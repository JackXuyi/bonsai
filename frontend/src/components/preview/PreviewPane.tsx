"use client";

// 表情页按钮区（旧 tab-preview）：瞬态表情 + 形态演示 + 提示。
// 都只是看看：表情走 face_preview（Rust 状态机会推回真实态），
// 形态演示只覆盖本地渲染（stagePreview），不碰真实成长状态。
// 大脸由 /face 页渲染，本组件只出按钮排。
import { facePreview } from "@/lib/commands";
import { inTauri } from "@/lib/tauriInvoke";
import { useUiStore } from "@/lib/stores/uiStore";
import { FACE_STATES } from "@/lib/types";

const STATE_LABELS: Record<string, string> = {
  calm: "平静",
  focused: "专注",
  sleepy: "困",
  greeted: "被搭话",
  displeased: "不高兴",
  thinking: "思考",
  proud: "得意",
};

const STAGE_LABELS = ["破土", "抽枝", "成形", "成景"];

export function PreviewPane() {
  const faceState = useUiStore((s) => s.faceState);
  const setFaceState = useUiStore((s) => s.setFaceState);
  const growth = useUiStore((s) => s.growth);
  const stagePreview = useUiStore((s) => s.stagePreview);
  const setStagePreview = useUiStore((s) => s.setStagePreview);

  function pickState(s: string) {
    setFaceState(s);
    if (inTauri) facePreview(s).catch(() => {});
  }

  function pickStage(stage: number) {
    setStagePreview({
      stage,
      dormant: growth?.dormant ?? false,
    });
  }

  function pickDormant() {
    setStagePreview({
      stage: growth?.stage ?? 0,
      dormant: true,
    });
  }

  const activeStage = stagePreview?.stage ?? growth?.stage ?? 0;
  const activeDormant = stagePreview?.dormant ?? growth?.dormant ?? false;

  // 旧 syncStageDemo：未演示时高亮真实成长阶段，演示后高亮所选
  const stageBtnActive = (i: number) =>
    !activeDormant && activeStage === i;
  const dormantBtnActive = () => activeDormant;

  return (
    <>
      <div className="states">
        {FACE_STATES.map((s) => (
          <button
            key={s}
            className={faceState === s ? "active" : ""}
            onClick={() => pickState(s)}
          >
            {STATE_LABELS[s]}
          </button>
        ))}
      </div>
      <div className="states states-secondary">
        {STAGE_LABELS.map((label, i) => (
          <button
            key={i}
            className={stageBtnActive(i) ? "active" : ""}
            onClick={() => pickStage(i)}
          >
            {label}
          </button>
        ))}
        <button
          className={dormantBtnActive() ? "active" : ""}
          onClick={pickDormant}
        >
          休眠
        </button>
      </div>
      <p className="hint">
        点脸 = 打招呼。上面是瞬态表情，下面是形态演示，
        <br />
        都只是看看，不会改它真实的状态。
      </p>
    </>
  );
}

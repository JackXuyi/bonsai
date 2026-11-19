"use client";

// L6 face SVG（旧 index.html 的 svg 骨架 + main.js 的 renderFrame /
// applyStageToSvg / renderMicroLeaves / applyLeafTint 翻译为 JSX + props）。
// Rust 决定 state/stage/dormant，这里只画。
import { memo } from "react";
import {
  POSES,
  STAGE_LAYOUT,
  leafTintStyle,
  microLeafPositions,
} from "./frames";
import type { FaceState } from "@/lib/types";

interface BonsaiFaceProps {
  state: FaceState;
  /** 帧序号（0/1，父级 600ms 翻转驱动动画） */
  frame: number;
  stage: number;
  dormant: boolean;
  dayIndex?: number;
  seed?: number;
}

function BonsaiFaceImpl({
  state,
  frame,
  stage,
  dormant,
  dayIndex,
  seed,
}: BonsaiFaceProps) {
  const s = Math.max(0, Math.min(3, Number(stage) || 0));
  const pose = POSES[state]?.[frame % 2] ?? POSES.calm[0];
  const lay = STAGE_LAYOUT[s] ?? STAGE_LAYOUT[2];

  // 旧 renderFrame：休眠时嘴角拉平 + 整体下垂 4°（与 pose transform 叠加）。
  const mouthD = dormant ? `M92 ${82 + lay.shift} L108 ${lay.shift + 82}` : pose.mouth;
  const svgTransform = dormant
    ? `${pose.transform} rotate(-4deg)`
    : pose.transform;

  // 旧 applyStageToSvg 的分组可见性：S3 是叠在 S2 上的 overlay。
  const stageOn = (gs: number) => (s >= 3 ? gs === 2 || gs === 3 : gs === s);
  const micro = microLeafPositions(seed ?? 0, Math.min(Math.max(0, dayIndex ?? 0), 6));
  const tint = leafTintStyle(dayIndex, dormant);

  const styleVars = {
    ...tint,
    transform: svgTransform,
  } as React.CSSProperties;

  return (
    <svg
      viewBox="0 0 200 200"
      data-stage={s}
      data-dormant={dormant ? "true" : "false"}
      aria-hidden="true"
      style={styleVars}
    >
      <path className="pot" d="M70 150 h60 l-8 34 h-44 z" />
      <rect className="pot-rim" x="64" y="142" width="72" height="12" rx="4" />
      {/* S0 破土 (day 0–1): a sprout, two cotyledons */}
      <g className={`stage-g${stageOn(0) ? " on" : ""}`} data-s="0">
        <path className="trunk" d="M100 142 C 100 132 100 124 100 116" />
        <circle className="leaf leaf-b" cx="80" cy="106" r="12" />
        <circle className="leaf leaf-c" cx="120" cy="106" r="12" />
      </g>
      {/* S1 抽枝 (day 2–4): a young tree, side branch + small canopy */}
      <g className={`stage-g${stageOn(1) ? " on" : ""}`} data-s="1">
        <path
          className="trunk"
          d="M96 142 C 96 124 95 112 92 100 M93 108 C 82 104 74 98 68 90"
        />
        <circle className="leaf leaf-a" cx="100" cy="86" r="24" />
        <circle className="leaf leaf-b" cx="64" cy="82" r="12" />
      </g>
      {/* S2 成形 (day 4–6): the full form the app always had */}
      <g className={`stage-g${stageOn(2) ? " on" : ""}`} data-s="2">
        <path
          className="trunk"
          d="M96 142 C 96 120 94 108 90 96 M90 104 C 78 100 72 94 68 86"
        />
        <circle className="leaf leaf-a" cx="100" cy="66" r="34" />
        <circle className="leaf leaf-b" cx="66" cy="76" r="20" />
        <circle className="leaf leaf-c" cx="134" cy="76" r="20" />
        <circle className="leaf leaf-d" cx="100" cy="94" r="22" />
      </g>
      {/* S3 成景 (day 7+): flowers + moss on top of the full form */}
      <g className={`stage-g stage-overlay${stageOn(3) ? " on" : ""}`} data-s="3">
        <circle className="flower f-1" cx="82" cy="52" r="4.5" />
        <circle className="flower f-2" cx="118" cy="58" r="4" />
        <circle className="flower f-3" cx="100" cy="38" r="4" />
        <ellipse className="moss m-1" cx="80" cy="152" rx="9" ry="3" />
        <ellipse className="moss m-2" cx="106" cy="156" rx="7" ry="2.5" />
        <ellipse className="moss m-3" cx="92" cy="150" rx="5" ry="2" />
      </g>
      <g className="micro-leaves">
        {micro.map((m, i) => (
          <circle key={i} className="micro-leaf" cx={m.cx} cy={m.cy} r={5} />
        ))}
      </g>
      <g className="face-features" style={{ transform: `translateY(${lay.shift}px)` }}>
        <g className="eyes">
          <ellipse className="eye eye-l" cx="84" cy="62" rx={pose.eyes[0]} ry={pose.eyes[1]} />
          <ellipse className="eye eye-r" cx="116" cy="62" rx={pose.eyes[2]} ry={pose.eyes[3]} />
        </g>
        <path className="mouth" d={mouthD} />
      </g>
    </svg>
  );
}

export const BonsaiFace = memo(BonsaiFaceImpl);

// 脸的帧数据与形态线常量（旧 ui/main.js 顶部常量段原样平移，不重写）。
// Rust 拥有状态与策略（face:state / growth:stage），这里只画。
import type { GrowthStatus } from "@/lib/types";

export const CAPTIONS: Record<string, string> = {
  calm: "平静地在窗台上待着",
  focused: "在看你写代码，不打扰",
  sleepy: "深夜了，话变少",
  greeted: "嘿，你来啦",
  displeased: "刚才那句，我有意见",
  thinking: "让我想想…",
  proud: "哼，我就说吧",
};

export const MOODS: Record<string, string> = {
  calm: "平静",
  focused: "专注",
  sleepy: "犯困",
  greeted: "回应中",
  displeased: "有意见",
  thinking: "思考",
  proud: "得意",
};

// 侧栏情绪点颜色（spec-m0b：情绪信息由情绪点承载，caption 取消）。
export const MOOD_COLORS: Record<string, string> = {
  calm: "#4c8c2b",
  focused: "#3f7024",
  sleepy: "#8a8468",
  greeted: "#d98cb3",
  displeased: "#b3552e",
  thinking: "#6a8fb3",
  proud: "#e0a53c",
};

// Stage captions (plan-morph.md §3.3): the self-narration sharpens as the
// tree grows. Pool picked by (day + hour-bucket) so it changes across days.
export const STAGE_CAPTIONS: Record<number, string[]> = {
  0: ["刚破土，别催我", "今天也还是小小的", "芽的事，急不来"],
  1: ["抽枝了，正在长", "枝头有点想法了", "长得还行，谢谢关心"],
  2: ["树形齐了，待着呢", "成形了，看你的了", "安静待着，有事叫我"],
  3: ["开花了，偶尔得意", "成景了，随便看", "盆景也要上班的"],
};

// Face-feature vertical placement per stage (the canopy moves up as the
// tree grows; features follow).
export const STAGE_LAYOUT: Record<number, { shift: number }> = {
  0: { shift: 40 },
  1: { shift: 22 },
  2: { shift: 0 },
  3: { shift: 0 },
};

export const MICRO_SLOTS: [number, number][] = [
  [150, 70],
  [48, 92],
  [142, 110],
  [62, 56],
  [128, 52],
  [52, 120],
];

export const POKE_LINES = [
  "阿盆抖了抖叶子",
  "阿盆摇了摇新枝",
  "阿盆把树冠晃了晃",
  "阿盆把花朝你转了转",
];

// Each state has exactly 2 frames: [eyeL rx,ry, eyeR rx,ry], mouth path, transform.
export const POSES: Record<string, { eyes: number[]; mouth: string; transform: string }[]> = {
  calm: [
    { eyes: [6, 8, 6, 8], mouth: "M92 82 Q100 88 108 82", transform: "translateY(0px)" },
    { eyes: [6, 8, 6, 8], mouth: "M92 82 Q100 88 108 82", transform: "translateY(-3px)" },
  ],
  focused: [
    { eyes: [6, 4, 6, 4], mouth: "M94 84 Q100 87 106 84", transform: "translateY(0px)" },
    { eyes: [6, 4, 6, 4], mouth: "M94 84 Q100 87 106 84", transform: "translateY(0px)" },
  ],
  sleepy: [
    { eyes: [6, 2, 6, 2], mouth: "M94 85 Q100 88 106 85", transform: "rotate(-2deg)" },
    { eyes: [6, 2, 6, 2], mouth: "M94 85 Q100 88 106 85", transform: "rotate(2deg)" },
  ],
  greeted: [
    { eyes: [7, 9, 7, 9], mouth: "M90 80 Q100 90 110 80", transform: "translateY(-6px)" },
    { eyes: [7, 8, 7, 8], mouth: "M90 81 Q100 89 110 81", transform: "translateY(0px)" },
  ],
  displeased: [
    { eyes: [6, 6, 6, 6], mouth: "M92 86 Q100 79 108 86", transform: "translateX(0px)" },
    { eyes: [6, 6, 6, 6], mouth: "M92 86 Q100 79 108 86", transform: "translateX(2px)" },
  ],
  thinking: [
    { eyes: [6, 8, 6, 3], mouth: "M93 85 L107 85", transform: "rotate(0deg)" },
    { eyes: [6, 8, 6, 3], mouth: "M93 85 L107 85", transform: "rotate(4deg)" },
  ],
  proud: [
    { eyes: [7, 7, 7, 7], mouth: "M88 80 Q100 92 112 80", transform: "translateY(-4px)" },
    { eyes: [7, 6, 7, 6], mouth: "M88 80 Q100 92 112 80", transform: "translateY(0px)" },
  ],
};

// ---- 叶色（旧 mixColor / TINT_* 平移） --------------------------------

function mixColor(a: string, b: string, t: number): string {
  const pa = [1, 3, 5].map((i) => parseInt(a.slice(i, i + 2), 16));
  const pb = [1, 3, 5].map((i) => parseInt(b.slice(i, i + 2), 16));
  return (
    "#" +
    pa
      .map((v, i) =>
        Math.round(v + (pb[i] - v) * t)
          .toString(16)
          .padStart(2, "0")
      )
      .join("")
  );
}

const TINT_FROM: Record<string, string> = {
  "--leaf": "#4c8c2b",
  "--leaf-dark": "#3a6e21",
  "--leaf-light": "#6aa846",
};
const TINT_TO: Record<string, string> = {
  "--leaf": "#3a6e21",
  "--leaf-dark": "#2e5a19",
  "--leaf-light": "#4c8c2b",
};
const TINT_DORMANT: Record<string, string> = {
  "--leaf": "#9aa85e",
  "--leaf-dark": "#87945a",
  "--leaf-light": "#b3ad6d",
};

/** 叶色 CSS 变量（inline style 会盖过 CSS 的 data-dormant 规则，与旧行为一致）。 */
export function leafTintStyle(day: number | undefined, dormant: boolean) {
  const t = Math.min(day ?? 0, 7) / 7;
  const vars: Record<string, string> = {};
  for (const k of Object.keys(TINT_FROM)) {
    vars[k] = dormant ? TINT_DORMANT[k] : mixColor(TINT_FROM[k], TINT_TO[k], t);
  }
  return vars;
}

/** 微叶位置：确定性伪随机（32-bit 切片，同旧 renderMicroLeaves）。 */
export function microLeafPositions(
  seed: number,
  count: number
): { cx: number; cy: number }[] {
  const used = new Set<number>();
  const out: { cx: number; cy: number }[] = [];
  let s = (Number(seed) >>> 0) || 1;
  for (let i = 0; i < count; i++) {
    s = (s * 1103515245 + 12345) >>> 0;
    let idx = s % MICRO_SLOTS.length;
    let tries = 0;
    while (used.has(idx) && tries < MICRO_SLOTS.length) {
      idx = (idx + 1) % MICRO_SLOTS.length;
      tries++;
    }
    if (used.has(idx)) break;
    used.add(idx);
    const [cx, cy] = MICRO_SLOTS[idx];
    out.push({ cx, cy });
  }
  return out;
}

/** 舞台 caption：基础态用成长旁白，瞬态用自己的台词（旧 growthCaptionText）。 */
export function growthCaptionText(growth: GrowthStatus | null): string | null {
  if (!growth || typeof growth.stage !== "number") return null;
  if (growth.dormant) return "好久没人理我了……";
  const pool = STAGE_CAPTIONS[growth.stage];
  if (!pool) return null;
  const hour = new Date().getHours();
  const bucket = hour < 6 ? 2 : hour < 12 ? 0 : hour < 18 ? 1 : 2;
  return pool[((growth.day_index ?? 0) + bucket) % pool.length];
}

/** dialogue outcome → 气泡徽注（旧 noteFor 原样平移）。 */
export function noteFor(outcome: {
  crisis?: boolean;
  tool?: boolean;
  model_used?: boolean;
  filter?: { unconditional_affirmation?: boolean; hedged_disagreement?: boolean };
}): string {
  if (outcome.crisis) return "危机资源 · 常规回复已拦截";
  // Tool answers come from the calendar cache, not the model — labelling them
  // "model unavailable" would be a lie.
  if (outcome.tool) return "日程 · 直接查的日历";
  const notes: string[] = [];
  if (!outcome.model_used) notes.push("本地模型未就绪");
  if (outcome.filter?.unconditional_affirmation)
    notes.push("反谄媚过滤：拦截了无条件肯定");
  if (outcome.filter?.hedged_disagreement) notes.push("反谄媚过滤：命中了缓冲式反对");
  return notes.join(" · ");
}

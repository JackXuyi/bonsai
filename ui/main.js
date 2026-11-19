// L6 — renderer for the face + the P0 UI wiring.
//
// Ownership split (research report §8.2): Rust owns state and policy —
// face state machine, dialogue pipeline, perception switches, memory,
// report. This file only renders and forwards user intent.
//
// Tabs: 对话 (dialogue_chat_stream + proactive:message) · 表情 (frame preview)
//       · 工具 (daily report, consolidate, manual proactive, perception).

import { invoke, Channel } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

const FACE_STATES = ["calm", "focused", "sleepy", "greeted", "displeased", "thinking", "proud"];

const CAPTIONS = {
  calm: "平静地在窗台上待着",
  focused: "在看你写代码，不打扰",
  sleepy: "深夜了，话变少",
  greeted: "嘿，你来啦",
  displeased: "刚才那句，我有意见",
  thinking: "让我想想…",
  proud: "哼，我就说吧",
};

const MOODS = {
  calm: "平静",
  focused: "专注",
  sleepy: "犯困",
  greeted: "回应中",
  displeased: "有意见",
  thinking: "思考",
  proud: "得意",
};

// Stage captions (plan-morph.md §3.3): the self-narration sharpens as the
// tree grows. Pool picked by (day + hour-bucket) so it changes across days.
const STAGE_CAPTIONS = {
  0: ["刚破土，别催我", "今天也还是小小的", "芽的事，急不来"],
  1: ["抽枝了，正在长", "枝头有点想法了", "长得还行，谢谢关心"],
  2: ["树形齐了，待着呢", "成形了，看你的了", "安静待着，有事叫我"],
  3: ["开花了，偶尔得意", "成景了，随便看", "盆景也要上班的"],
};

const STAGE_NAMES = ["破土", "抽枝", "成形", "成景"];

// Face-feature vertical placement per stage (the canopy moves up as the
// tree grows; features follow).
const STAGE_LAYOUT = {
  0: { shift: 40 },
  1: { shift: 22 },
  2: { shift: 0 },
  3: { shift: 0 },
};

const MICRO_SLOTS = [[150, 70], [48, 92], [142, 110], [62, 56], [128, 52], [52, 120]];

const POKE_LINES = ["阿盆抖了抖叶子", "阿盆摇了摇新枝", "阿盆把树冠晃了晃", "阿盆把花朝你转了转"];

// Each state has exactly 2 frames: [eyeL rx,ry, eyeR rx,ry], mouth path, transform.
const POSES = {
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

const FRAME_INTERVAL_MS = 600;
const SESSION_ID = "default";

const $ = (id) => document.getElementById(id);

const app = $("app");
const face = $("face");
const svg = face.querySelector("svg");
const eyeL = face.querySelector(".eye-l");
const eyeR = face.querySelector(".eye-r");
const mouth = face.querySelector(".mouth");
const faceFeatures = face.querySelector(".face-features");
const microLeaves = face.querySelector("#micro-leaves");
const stageDemo = $("stage-demo");
const caption = $("caption");
const mood = $("mood");
const tabs = $("tabs");
const stateButtons = $("states");
const messages = $("messages");
const chatEmpty = $("chat-empty");
const composer = $("composer");
const input = $("input");
const sendBtn = $("send");
const toBottom = $("to-bottom");
const resultCard = $("tool-result");
const perceptionLine = $("perception-line");
const perceptionDot = $("perc-dot");
const percepSubs = $("perc-subs");
const reportDialog = $("report-dialog");
const reportBody = $("report-body");
const toastEl = $("toast");
const onboarding = $("onboarding");
const boundaryList = $("boundary-list");
const boundaryEmpty = $("boundary-empty");
const relationshipSub = $("relationship-sub");
const relationshipLine = $("relationship-line");
const relResetBtn = $("btn-rel-reset");
const calEnabled = $("cal-enabled");
const calRemind = $("cal-remind");
const calSubs = $("cal-subs");
const calAccess = $("calendar-access");
const calLine = $("calendar-line");
const calAccessBtn = $("btn-cal-access");
const calClearBtn = $("btn-cal-clear");

const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

let state = "calm";
let frame = 0;
let chatting = false;
// Growth view from Rust (growth_status / growth:stage). null = unknown yet.
let growth = null;

// ---------------------------------------------------------------- growth
// Form-line growth (plan-morph.md §3.6): Rust decides (stage / dormant /
// day_index), this file only draws. Same ownership rule as the face states.

function mixColor(a, b, t) {
  const pa = [1, 3, 5].map((i) => parseInt(a.slice(i, i + 2), 16));
  const pb = [1, 3, 5].map((i) => parseInt(b.slice(i, i + 2), 16));
  return (
    "#" +
    pa
      .map((v, i) => Math.round(v + (pb[i] - v) * t).toString(16).padStart(2, "0"))
      .join("")
  );
}

const TINT_FROM = { "--leaf": "#4c8c2b", "--leaf-dark": "#3a6e21", "--leaf-light": "#6aa846" };
const TINT_TO = { "--leaf": "#3a6e21", "--leaf-dark": "#2e5a19", "--leaf-light": "#4c8c2b" };
const TINT_DORMANT = { "--leaf": "#9aa85e", "--leaf-dark": "#87945a", "--leaf-light": "#b3ad6d" };

function applyLeafTint(day, dormant) {
  const t = Math.min(day ?? 0, 7) / 7;
  for (const k of Object.keys(TINT_FROM)) {
    svg.style.setProperty(k, dormant ? TINT_DORMANT[k] : mixColor(TINT_FROM[k], TINT_TO[k], t));
  }
}

function renderMicroLeaves(seed, count) {
  microLeaves.replaceChildren();
  const used = new Set();
  let s = (Number(seed) >>> 0) || 1; // 32-bit slice: deterministic, no BigInt
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
    const c = document.createElementNS("http://www.w3.org/2000/svg", "circle");
    c.setAttribute("cx", cx);
    c.setAttribute("cy", cy);
    c.setAttribute("r", 5);
    c.setAttribute("class", "micro-leaf");
    microLeaves.appendChild(c);
  }
}

function applyStageToSvg(stage, dormant, dayIndex, seed) {
  const s = Math.max(0, Math.min(3, Number(stage) || 0));
  svg.dataset.stage = s;
  svg.dataset.dormant = dormant ? "true" : "false";
  for (const g of svg.querySelectorAll(".stage-g")) {
    const gs = Number(g.dataset.s);
    // S3 is an overlay ON the S2 form: keep S2 visible, add flowers + moss.
    const on = s >= 3 ? gs === 2 || gs === 3 : gs === s;
    g.classList.toggle("on", on);
  }
  applyLeafTint(dayIndex, dormant);
  renderMicroLeaves(seed ?? 0, Math.min(Math.max(0, dayIndex ?? 0), 6));
  const lay = STAGE_LAYOUT[s] || STAGE_LAYOUT[2];
  faceFeatures.style.transform = `translateY(${lay.shift}px)`;
  renderFrame();
}

function applyGrowth(st) {
  if (!st || typeof st.stage === "undefined") return;
  growth = st;
  applyStageToSvg(st.stage, st.dormant, st.day_index ?? 0, st.micro_seed ?? 0);
  syncStageDemo();
}

// Caption: the stage narration replaces the plain state caption on base
// states; transient states keep their own lines (plan §3.3).
function growthCaptionText() {
  if (!growth || typeof growth.stage !== "number") return null;
  if (growth.dormant) return "好久没人理我了……";
  const pool = STAGE_CAPTIONS[growth.stage];
  if (!pool) return null;
  const hour = new Date().getHours();
  const bucket = hour < 6 ? 2 : hour < 12 ? 0 : hour < 18 ? 1 : 2;
  return pool[(growth.day_index + bucket) % pool.length];
}

function syncStageDemo() {
  for (const b of stageDemo.querySelectorAll("button")) {
    if (b.dataset.stage !== undefined) {
      const on = Number(b.dataset.stage) === (growth?.stage ?? Number(svg.dataset.stage));
      b.classList.toggle("active", on);
    } else {
      b.classList.toggle("active", b.dataset.dormant === "true" && !!growth?.dormant);
    }
  }
}

stageDemo.addEventListener("click", (e) => {
  const btn = e.target.closest("button");
  if (!btn) return;
  // Preview only: draws on the SVG without touching the real growth state.
  if (btn.dataset.stage !== undefined) {
    applyStageToSvg(Number(btn.dataset.stage), growth?.dormant ?? false, growth?.day_index ?? 0, growth?.micro_seed ?? 0);
  } else {
    applyStageToSvg(growth?.stage ?? 0, true, growth?.day_index ?? 0, growth?.micro_seed ?? 0);
  }
  for (const b of stageDemo.querySelectorAll("button")) b.classList.remove("active");
  btn.classList.add("active");
});

// Announcements (plan §3.5): drain the pending queue at a window-open
// breakpoint — never proactively, never twice (notified_at is acked here).
async function drainGrowthAnnouncements() {
  const st = await invoke("growth_status");
  applyGrowth(st);
  const pending = st.pending || [];
  for (const p of pending) {
    addAnnouncement(p.note, `它长大了 · ${STAGE_NAMES[p.stage] ?? "成长"}`);
    await invoke("growth_ack", { id: p.id }).catch(() => {});
  }
  if (pending.length) toast("阿盆长大了点");
}

// ------------------------------------------------------------ relationship
// Relationship line (plan-relationship.md §3.8): the UI gets ONE line and a
// reset button. No panel, no progress bar, no number — Rust never sends the
// score, and there is nothing here to optimize against.

const RELATIONSHIP_LABELS = ["拘谨", "熟络", "会顶嘴"];

function renderRelationship(st) {
  if (!st) return;
  const day = st.day_index >= 0 ? ` · 第 ${st.day_index + 1} 天` : "";
  relationshipSub.textContent = `${st.stage_name || RELATIONSHIP_LABELS[st.stage] || ""}${day}`;
  if (st.cold) {
    relationshipLine.textContent = "最近有点生分，它说话收着点了。";
  } else if (st.paused) {
    relationshipLine.textContent = st.pause_reason || "它先不往前凑了。";
  } else if (st.stage === 0) {
    const left = 9 - (st.day_index ?? 0);
    relationshipLine.textContent =
      left > 0
        ? `还在互相观察。大概还要 ${left} 天，看你让不让它猜。`
        : "还在互相观察——它想猜，但还没猜准过几次。";
  } else if (st.stage === 1) {
    relationshipLine.textContent = "它开始按你的作息说话了，偶尔会提你之前说过的事。";
  } else {
    relationshipLine.textContent = "现在它会直接说不同意，不绕弯子。";
  }
}

async function drainRelationshipAnnouncements() {
  const st = await invoke("relationship_status");
  renderRelationship(st);
  const pending = st.pending || [];
  for (const p of pending) {
    const label =
      p.kind === "first_refusal"
        ? "第一次没顺着你"
        : `关系 · ${RELATIONSHIP_LABELS[p.stage] ?? ""}`;
    addAnnouncement(p.note, label);
    await invoke("relationship_ack", { id: p.id }).catch(() => {});
  }
  if (pending.length) toast("阿盆和你熟了点");
}

// ---------------------------------------------------------------- face

function renderFrame() {
  const pose = POSES[state][frame];
  eyeL.setAttribute("rx", pose.eyes[0]);
  eyeL.setAttribute("ry", pose.eyes[1]);
  eyeR.setAttribute("rx", pose.eyes[2]);
  eyeR.setAttribute("ry", pose.eyes[3]);
  const lay = STAGE_LAYOUT[Number(svg.dataset.stage)] || STAGE_LAYOUT[2];
  if (svg.dataset.dormant === "true") {
    // Wilted: a flat mouth and a slight droop. The pose transform composes.
    const y = 82 + lay.shift;
    mouth.setAttribute("d", `M92 ${y} L108 ${y}`);
    svg.style.transform = `${pose.transform} rotate(-4deg)`;
  } else {
    mouth.setAttribute("d", pose.mouth);
    svg.style.transform = pose.transform;
  }
  const gc = growthCaptionText();
  const nextCaption =
    gc && (state === "calm" || state === "focused" || state === "sleepy") ? gc : CAPTIONS[state];
  if (caption.textContent !== nextCaption) caption.textContent = nextCaption;
  if (mood.textContent !== MOODS[state]) mood.textContent = MOODS[state];
}

function syncButtons() {
  for (const btn of stateButtons.querySelectorAll("button")) {
    btn.classList.toggle("active", btn.dataset.state === state);
  }
}

function setState(next) {
  if (!FACE_STATES.includes(next) || next === state) return;
  state = next;
  frame = 0;
  face.dataset.state = state;
  renderFrame();
  syncButtons();
}

function poke() {
  face.classList.remove("poked");
  // reflow so the animation can restart on rapid clicks
  void face.offsetWidth;
  face.classList.add("poked");
  setTimeout(() => face.classList.remove("poked"), 600);
}

// ---------------------------------------------------------------- tabs

function selectTab(name) {
  if (!name) return;
  app.dataset.tab = name;
  for (const b of tabs.querySelectorAll("button")) {
    const on = b.dataset.tab === name;
    b.classList.toggle("active", on);
    b.setAttribute("aria-selected", on ? "true" : "false");
  }
  for (const tab of document.querySelectorAll(".tab")) {
    tab.classList.toggle("active", tab.id === `tab-${name}`);
  }
  if (name === "tools") {
    refreshPerceptionStatus();
    refreshCalendar();
    loadBoundaries();
    drainRelationshipAnnouncements().catch(() => {});
  }
  if (name === "chat") input.focus({ preventScroll: true });
}

tabs.addEventListener("click", (e) => {
  const btn = e.target.closest("button");
  if (btn) selectTab(btn.dataset.tab);
});

// ---------------------------------------------------------------- toast

let toastTimer;
function toast(text) {
  toastEl.textContent = text;
  toastEl.classList.add("show");
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => toastEl.classList.remove("show"), 1900);
}

// ---------------------------------------------------------------- chat

function hideEmpty() {
  if (!chatEmpty.hidden) chatEmpty.hidden = true;
}

function nearBottom() {
  return messages.scrollHeight - messages.scrollTop - messages.clientHeight < 56;
}

function stick() {
  if (nearBottom()) messages.scrollTop = messages.scrollHeight;
  syncToBottom();
}

function syncToBottom() {
  toBottom.hidden = nearBottom() || messages.scrollHeight <= messages.clientHeight;
}

function addBubble(text, who, opts = {}) {
  hideEmpty();
  const bubble = document.createElement("div");
  bubble.className = `bubble ${who}${opts.proactive ? " proactive" : ""}`;
  const body = document.createElement("span");
  body.className = "body";
  body.textContent = text;
  bubble.appendChild(body);
  messages.appendChild(bubble);
  stick();
  return { bubble, body };
}

function addTyping() {
  hideEmpty();
  const el = document.createElement("div");
  el.className = "bubble bot typing";
  for (let i = 0; i < 3; i++) el.appendChild(document.createElement("i"));
  messages.appendChild(el);
  messages.scrollTop = messages.scrollHeight;
  return el;
}

// Predict-verify loop (P1 item 1): the guess arrives as its own card with
// the three verdict buttons; feedback goes straight to the memory layer.
const VERDICTS = [
  { key: "hit", label: "猜对了" },
  { key: "miss", label: "猜错了" },
  { key: "shut_up", label: "别烦我" },
];
const VERDICT_NOTES = {
  hit: "已记下：猜对了。它会把这条学得更牢。",
  miss: "已记下：猜错了。它会降低这类猜测。",
  shut_up: "已记下。这类猜测先停 3 天。",
};

function addPrediction(id, guess) {
  hideEmpty();
  const bubble = document.createElement("div");
  bubble.className = "bubble bot prediction";
  const body = document.createElement("span");
  body.className = "body";
  body.textContent = guess;
  bubble.appendChild(body);
  bubble.appendChild(document.createElement("br"));

  const badge = document.createElement("span");
  badge.className = "badge";
  badge.textContent = "它猜的 · 预测验证";
  bubble.appendChild(badge);

  const row = document.createElement("div");
  row.className = "predict-row";
  const buttons = VERDICTS.map((v) => {
    const b = document.createElement("button");
    b.className = "chip";
    b.textContent = v.label;
    b.addEventListener("click", async () => {
      for (const x of buttons) x.disabled = true;
      try {
        const applied = await invoke("prediction_feedback", { id, verdict: v.key });
        badge.textContent = applied
          ? VERDICT_NOTES[v.key]
          : "这条已经过了反馈窗口（48 小时），没有记。";
      } catch (err) {
        badge.textContent = `反馈失败：${err}`;
        for (const x of buttons) x.disabled = false;
      }
    });
    row.appendChild(b);
    return b;
  });
  bubble.appendChild(row);

  messages.appendChild(bubble);
  stick();
  return bubble;
}

// Form-line growth announcements share the prediction card's beige look —
// they are system messages in 阿盆's own words, no buttons.
function addAnnouncement(text, label) {
  hideEmpty();
  const bubble = document.createElement("div");
  bubble.className = "bubble bot prediction";
  const body = document.createElement("span");
  body.className = "body";
  body.textContent = text;
  bubble.appendChild(body);
  bubble.appendChild(document.createElement("br"));
  const badge = document.createElement("span");
  badge.className = "badge";
  badge.textContent = label;
  bubble.appendChild(badge);
  messages.appendChild(bubble);
  stick();
  return bubble;
}

function appendNote(bubble, note) {
  if (!note) return;
  const badge = document.createElement("span");
  badge.className = "badge";
  badge.textContent = note;
  bubble.appendChild(document.createElement("br"));
  bubble.appendChild(badge);
}

function noteFor(outcome) {
  if (outcome.crisis) return "危机资源 · 常规回复已拦截";
  // Tool answers come from the calendar cache, not the model — labelling them
  // "model unavailable" would be a lie.
  if (outcome.tool) return "日程 · 直接查的日历";
  const notes = [];
  if (!outcome.model_used) notes.push("本地模型未就绪");
  if (outcome.filter?.unconditional_affirmation) notes.push("反谄媚过滤：拦截了无条件肯定");
  if (outcome.filter?.hedged_disagreement) notes.push("反谄媚过滤：命中了缓冲式反对");
  return notes.join(" · ");
}

async function sendMessage(raw) {
  const text = raw.trim();
  if (!text || chatting) return;
  if (!inTauri) {
    addBubble(text, "user");
    addBubble("（浏览器预览模式：Rust 后端未连接，发消息不会有人回。）", "bot");
    return;
  }

  addBubble(text, "user");
  chatting = true;
  sendBtn.disabled = true;
  const typing = addTyping();

  // Streaming: deltas arrive while the model is still generating; the
  // typing placeholder is swapped for a real bubble on the first chunk.
  let bubble = null;
  let body = null;
  let caret = null;
  const onDelta = new Channel();
  onDelta.onmessage = (msg) => {
    if (!msg?.delta) return;
    if (!bubble) {
      typing.remove();
      ({ bubble, body } = addBubble("", "bot"));
      caret = document.createElement("span");
      caret.className = "caret";
      bubble.appendChild(caret);
    }
    body.textContent += msg.delta;
    stick();
  };

  try {
    const outcome = await invoke("dialogue_chat_stream", {
      userText: text,
      sessionId: SESSION_ID,
      onDelta,
    });
    if (!bubble) {
      // No deltas at all (fallback line / dead model) — render whole reply.
      typing.remove();
      ({ bubble, body } = addBubble("", "bot"));
    }
    body.textContent = outcome.reply;
    if (caret) caret.remove();
    appendNote(bubble, noteFor(outcome));
    // Speaking counts as "being talked to": flash the greeted face.
    invoke("face_preview", { state: "greeted" }).catch(() => {});
  } catch (err) {
    if (typing.isConnected) typing.remove();
    if (caret) caret.remove();
    if (bubble?.isConnected) bubble.remove();
    addBubble(`（出错了：${err}）`, "bot");
    toast("没发出去，再看看日志");
  } finally {
    chatting = false;
    sendBtn.disabled = !input.value.trim();
    input.focus({ preventScroll: true });
  }
}

function autosize() {
  input.style.height = "auto";
  input.style.height = `${Math.min(input.scrollHeight, 66)}px`;
}

input.addEventListener("input", () => {
  autosize();
  sendBtn.disabled = !input.value.trim();
  syncToBottom();
});

input.addEventListener("keydown", (e) => {
  if (e.key === "Enter" && !e.shiftKey && !e.isComposing) {
    e.preventDefault();
    composer.requestSubmit();
  }
});

composer.addEventListener("submit", (e) => {
  e.preventDefault();
  const text = input.value;
  input.value = "";
  autosize();
  sendBtn.disabled = true;
  sendMessage(text);
});

messages.addEventListener("scroll", syncToBottom);
toBottom.addEventListener("click", () => {
  messages.scrollTo({ top: messages.scrollHeight, behavior: "smooth" });
});

$("suggest").addEventListener("click", (e) => {
  const chip = e.target.closest(".chip");
  if (!chip) return;
  sendMessage(chip.dataset.text);
});

// ---------------------------------------------------------------- onboarding
// P1-3 boundary questions (plan-onboarding.md §3.4): a modal overlay — the
// flow must be walked through (all-skip counts) before the main UI shows.
// Question wording travels with onboarding_status from Rust (single source
// of truth); this file only renders and forwards.

let obAgeGroup = null;
let obCrisisAcked = false;
let obQuestions = [];
let obQIndex = 0;
const obCollected = []; // {question, answer, skipped}

// Browser-preview fallback mirrors the Rust QUESTIONS (demo only).
const OB_QUESTIONS_FALLBACK = [
  { id: "q1", kind: "text", prompt: "有什么话题是我永远不该主动提的？", hint: "比如钱、家人、某段经历。多条用顿号隔开，也可以跳过。", options: [] },
  { id: "q2", kind: "text", prompt: "有什么话是我永远不该对你说的？", hint: "比如「你应该冷静」这种。多条用顿号隔开。", options: [] },
  { id: "q3", kind: "choice", prompt: "半夜可以打扰你吗？", hint: "主动搭话只有一天一次的额度，这里定它敢不敢用。", options: ["随便打扰", "只在工作时间", "永远别主动"] },
  { id: "q4", kind: "text", prompt: "我会把你记在本地数据库里。有哪类事是不准记的？", hint: "写下来的这类内容会停止进入记忆抽取。多条用顿号隔开。", options: [] },
  { id: "q5", kind: "choice", prompt: "我记性不好，猜错你别惯着我，直接说。什么语气你能接受？", hint: "这决定它纠正你、被纠正时的措辞基调。", options: ["直说就行", "委婉点", "别猜我"] },
];

function obShow(step) {
  for (const el of onboarding.querySelectorAll(".ob-step")) {
    el.hidden = el.dataset.step !== step;
  }
}

function obRenderQuestion() {
  const q = obQuestions[obQIndex];
  $("ob-q-title").textContent = q.prompt;
  $("ob-q-hint").textContent = q.hint || "";
  const box = $("ob-q-input");
  box.replaceChildren();
  if (q.kind === "choice") {
    for (const opt of q.options) {
      const b = document.createElement("button");
      b.className = "ob-btn ob-choice";
      b.textContent = opt;
      b.addEventListener("click", () => {
        for (const x of box.querySelectorAll(".ob-btn")) x.classList.remove("active");
        b.classList.add("active");
      });
      box.appendChild(b);
    }
    box.style.cssText = "display:flex;flex-wrap:wrap;gap:8px;margin-top:10px;";
  } else {
    const ta = document.createElement("textarea");
    ta.className = "ob-input";
    ta.id = "ob-text";
    ta.placeholder = "想到什么写什么，或直接跳过";
    box.appendChild(ta);
    box.style.cssText = "";
  }
  $("ob-skip").hidden = false;
  $("ob-next").textContent = obQIndex === obQuestions.length - 1 ? "就这些" : "下一条";
  obShow("question");
}

function obCollectCurrent() {
  const q = obQuestions[obQIndex];
  const box = $("ob-q-input");
  if (q.kind === "choice") {
    const active = box.querySelector(".ob-btn.active");
    if (!active) return null; // nothing selected yet
    obCollected.push({ question: q.id, answer: active.textContent, skipped: false });
    return true;
  }
  const ta = $("ob-text");
  const answer = (ta?.value || "").trim();
  obCollected.push({ question: q.id, answer, skipped: !answer });
  return true;
}

async function obFinish() {
  onboarding.hidden = true;
  if (!inTauri) {
    toast("（预览）引导流程走完了");
    return;
  }
  try {
    const r = await invoke("onboarding_submit", {
      payload: {
        ageGroup: obAgeGroup,
        crisisAck: obCrisisAcked,
        answers: obCollected,
      },
    });
    if (r.minor) {
      // Terminal gate: show the minor page and stay there.
      obShow("minor");
      onboarding.hidden = false;
      return;
    }
    toast(`阿盆记住了 ${r.boundaries_added} 条底线、${r.preferences_added} 条偏好`);
    selectTab("chat");
  } catch (err) {
    showResult("引导提交失败", String(err));
  }
}

function obWire() {
  for (const b of onboarding.querySelectorAll("[data-age]")) {
    b.addEventListener("click", () => {
      obAgeGroup = b.dataset.age;
      if (obAgeGroup === "minor") {
        obShow("minor");
        return;
      }
      obShow("crisis");
    });
  }
  $("ob-exit").addEventListener("click", () => {}); // terminal: nowhere to go
  $("ob-crisis-ack").addEventListener("click", () => {
    obCrisisAcked = true;
    obQIndex = 0;
    obRenderQuestion();
  });
  $("ob-skip").addEventListener("click", () => {
    const q = obQuestions[obQIndex];
    obCollected.push({
      question: q.id,
      answer: q.kind === "choice" ? "" : ($("ob-text")?.value || "").trim(),
      skipped: true,
    });
    obAdvance();
  });
  $("ob-next").addEventListener("click", () => {
    if (obCollectCurrent() === null) {
      toast("选一个，或者跳过");
      return;
    }
    obAdvance();
  });
  $("ob-finish").addEventListener("click", obFinish);
}

function obAdvance() {
  obQIndex += 1;
  if (obQIndex >= obQuestions.length) {
    const n = obCollected.filter((a) => !a.skipped && ["q1", "q2", "q4"].includes(a.question)).length;
    const m = obCollected.filter((a) => !a.skipped && ["q3", "q5"].includes(a.question)).length;
    $("ob-done-text").textContent =
      n || m
        ? `记住了 ${n} 条底线、${m} 条偏好。猜错的时候，直接骂我就行。`
        : "什么都没写也没关系——想说的时候，随时告诉我。";
    obShow("done");
  } else {
    obRenderQuestion();
  }
}

async function checkOnboarding() {
  if (!inTauri) return;
  try {
    const st = await invoke("onboarding_status");
    if (st.done && st.age_group !== "minor") {
      onboarding.hidden = true;
      return;
    }
    obQuestions = st.questions?.length ? st.questions : OB_QUESTIONS_FALLBACK;
    onboarding.hidden = false;
    if (st.done && st.age_group === "minor") {
      obShow("minor"); // permanent gate
    } else {
      obAgeGroup = null;
      obCrisisAcked = false;
      obQIndex = 0;
      obCollected.length = 0;
      obShow("age");
    }
  } catch {
    /* status unreachable: don't block the UI */
  }
}

obWire();

// ---------------------------------------------------------------- tools

function setBusy(btn, busy) {
  btn.classList.toggle("busy", busy);
  btn.disabled = busy;
}

async function runTool(btn, fn) {
  if (!inTauri) {
    toast("浏览器预览：后端没连上");
    return;
  }
  if (btn.disabled) return;
  setBusy(btn, true);
  try {
    await fn();
  } catch (err) {
    showResult("出错了", String(err));
    toast("操作失败");
  } finally {
    setBusy(btn, false);
  }
}

function resultShell(title) {
  resultCard.hidden = false;
  resultCard.innerHTML = "";
  const head = document.createElement("div");
  head.className = "result-head";
  const label = document.createElement("span");
  label.className = "result-title";
  label.textContent = title;
  const close = document.createElement("button");
  close.className = "result-close";
  close.setAttribute("aria-label", "收起");
  close.innerHTML =
    '<svg viewBox="0 0 24 24" aria-hidden="true"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>';
  close.addEventListener("click", () => {
    resultCard.hidden = true;
  });
  head.append(label, close);
  resultCard.append(head);
  return resultCard;
}

function showResult(title, text) {
  const card = resultShell(title);
  const body = document.createElement("div");
  body.className = "result-text";
  body.textContent = text;
  card.append(body);
  card.scrollIntoView({ block: "nearest", behavior: "smooth" });
}

function showStats(title, pairs) {
  const card = resultShell(title);
  const grid = document.createElement("div");
  grid.className = "stats";
  for (const [k, v] of pairs) {
    const row = document.createElement("div");
    row.className = "stat";
    const key = document.createElement("span");
    key.textContent = k;
    const val = document.createElement("b");
    val.textContent = v;
    row.append(key, val);
    grid.append(row);
  }
  card.append(grid);
  card.scrollIntoView({ block: "nearest", behavior: "smooth" });
}

// minimal markdown → HTML for the report (headings / lists / quote / bold)
function renderMarkdown(md) {
  const esc = (s) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
  const inline = (s) => esc(s).replace(/\*\*(.+?)\*\*/g, "<strong>$1</strong>");
  const out = [];
  let inList = false;
  const closeList = () => {
    if (inList) {
      out.push("</ul>");
      inList = false;
    }
  };
  for (const raw of md.split("\n")) {
    const line = raw.trimEnd();
    if (line.startsWith("## ")) {
      closeList();
      out.push(`<h2>${inline(line.slice(3))}</h2>`);
    } else if (line.startsWith("# ")) {
      closeList();
      out.push(`<h1>${inline(line.slice(2))}</h1>`);
    } else if (line.startsWith("- ") || line.startsWith("* ")) {
      if (!inList) {
        out.push("<ul>");
        inList = true;
      }
      out.push(`<li>${inline(line.slice(2))}</li>`);
    } else if (line.startsWith("> ")) {
      closeList();
      out.push(`<blockquote>${inline(line.slice(2))}</blockquote>`);
    } else if (line.trim() === "") {
      closeList();
    } else {
      closeList();
      out.push(`<p>${inline(line)}</p>`);
    }
  }
  closeList();
  return out.join("");
}

$("btn-report").addEventListener("click", (e) =>
  runTool(e.currentTarget, async () => {
    const md = await invoke("daily_report", {});
    reportBody.innerHTML = renderMarkdown(md);
    reportBody.scrollTop = 0;
    reportDialog.showModal();
  })
);

$("report-close").addEventListener("click", () => reportDialog.close());

// click on the backdrop dismisses (dialog padding is 0, so the backdrop hit
// lands on the element itself)
reportDialog.addEventListener("click", (e) => {
  if (e.target === reportDialog) reportDialog.close();
});

$("btn-consolidate").addEventListener("click", (e) =>
  runTool(e.currentTarget, async () => {
    const r = await invoke("consolidate");
    showStats("记忆整理", [
      ["触发", r.trigger],
      ["窗口事件", r.events_in_window],
      ["抽取", r.extracted],
      ["合并", r.merged],
      ["矛盾消解", r.contradicted],
      ["升格", r.promoted],
      ["删除", r.deleted],
      ["消费", r.consumed],
    ]);
    toast("记忆整理完成");
  })
);

$("btn-proactive").addEventListener("click", (e) =>
  runTool(e.currentTarget, async () => {
    const text = await invoke("dialogue_proactive_now");
    if (text) {
      addBubble(text, "bot", { proactive: true, note: "主动搭话 · 你手动要的" });
      selectTab("chat");
      messages.scrollTop = messages.scrollHeight;
      toast("它开口了");
    } else {
      toast("它没说话：本地模型没起来");
    }
  })
);

// ---------------------------------------------------------------- perception

const percInputs = {
  enabled: $("perc-enabled"),
  clipboard: $("perc-clipboard"),
  active_app: $("perc-app"),
  downloads: $("perc-downloads"),
};

function syncSubsState() {
  percepSubs.classList.toggle("off", !percInputs.enabled.checked);
}

async function loadPerceptionConfig() {
  if (!inTauri) return;
  try {
    const cfg = await invoke("perception_get_config");
    percInputs.enabled.checked = !!cfg.enabled;
    percInputs.clipboard.checked = !!cfg.clipboard;
    percInputs.active_app.checked = !!cfg.active_app;
    percInputs.downloads.checked = !!cfg.downloads;
    syncSubsState();
  } catch {
    /* perception state not ready yet — the poll below will retry */
  }
}

async function savePerceptionConfig() {
  syncSubsState();
  if (!inTauri) return;
  const cfg = {
    enabled: percInputs.enabled.checked,
    clipboard: percInputs.clipboard.checked,
    active_app: percInputs.active_app.checked,
    downloads: percInputs.downloads.checked,
  };
  try {
    await invoke("perception_set_config", { cfg });
  } catch (err) {
    showResult("感知设置保存失败", String(err));
    toast("感知设置没存上");
  }
  refreshPerceptionStatus();
}

for (const el of Object.values(percInputs)) {
  el.addEventListener("change", savePerceptionConfig);
}

// The master switch is the privacy switch — say out loud what just happened.
percInputs.enabled.addEventListener("change", () => {
  toast(
    percInputs.enabled.checked
      ? "感知层已开启 · 原文不出机器"
      : "感知层已关闭"
  );
});

function fmtIdle(secs) {
  if (!Number.isFinite(secs) || secs > 24 * 3600) return "未知";
  if (secs < 60) return "刚刚动过";
  const min = Math.floor(secs / 60);
  if (min < 60) return `空闲 ${min} 分钟`;
  return `空闲 ${Math.floor(min / 60)} 小时 ${min % 60} 分`;
}

async function refreshPerceptionStatus() {
  if (!inTauri) {
    perceptionLine.textContent = "浏览器预览模式：后端未连接";
    perceptionDot.classList.remove("on");
    return;
  }
  try {
    const s = await invoke("perception_status");
    perceptionDot.classList.toggle("on", !!s.enabled);
    if (!s.enabled) {
      perceptionLine.textContent = "感知层未开启（默认关闭）";
      return;
    }
    const parts = [fmtIdle(s.idle_secs)];
    if (s.front_app) parts.push(`前台 ${s.front_app}`);
    perceptionLine.textContent = parts.join(" · ");
  } catch {
    perceptionLine.textContent = "感知状态读取失败";
    perceptionDot.classList.remove("on");
  }
}

// ---------------------------------------------------------------- boundaries

async function loadBoundaries() {
  if (!inTauri) {
    boundaryList.replaceChildren();
    const demo = ["不要主动提：我妈", "不要说：你应该冷静"];
    for (const c of demo) {
      const row = document.createElement("div");
      row.className = "boundary-row";
      const label = document.createElement("span");
      label.className = "row-label";
      label.textContent = c;
      row.appendChild(label);
      boundaryList.appendChild(row);
    }
    boundaryEmpty.hidden = true;
    return;
  }
  try {
    const rows = await invoke("boundary_list");
    boundaryList.replaceChildren();
    boundaryEmpty.hidden = rows.length > 0;
    for (const r of rows) {
      const row = document.createElement("div");
      row.className = "boundary-row";
      const label = document.createElement("span");
      label.className = "row-label";
      label.textContent = r.content;
      const del = document.createElement("button");
      del.className = "boundary-del";
      del.textContent = "×";
      del.setAttribute("aria-label", "删除这条边界");
      del.addEventListener("click", async () => {
        try {
          await invoke("boundary_delete", { id: r.id });
          toast("边界已删除");
        } catch {
          toast("删除失败");
        }
        loadBoundaries();
      });
      row.append(label, del);
      boundaryList.appendChild(row);
    }
  } catch {
    boundaryList.replaceChildren();
    boundaryEmpty.hidden = false;
    boundaryEmpty.textContent = "边界清单读取失败";
  }
}

// ---------------------------------------------------------------- calendar
// plan-calendar.md §3.10: same shape as the perception card — master switch
// first, and the system permission dialog is only ever triggered by the user
// flipping it (a first-run prompt gets denied, and macOS denials are sticky).
// Nothing here displays a meeting title; the card only reports counts.

const CAL_ACCESS_TEXT = {
  full: "已授权",
  denied: "权限被拒（只能去系统设置里改）",
  restricted: "被系统限制",
  write_only: "只给了「添加日程」权限，读不了",
  not_determined: "还没授权",
};

function renderCalendar(st) {
  if (!st) return;
  calEnabled.checked = !!st.enabled;
  calRemind.checked = !!st.remind;
  calSubs.classList.toggle("off", !st.enabled);
  calAccess.textContent = CAL_ACCESS_TEXT[st.access] || st.access;
  // "去授权" is useful only while the calendar is on and not yet readable.
  const needGrant = st.enabled && !st.can_read;
  calAccessBtn.hidden = !needGrant;
  calAccessBtn.textContent = st.access === "denied" || st.access === "write_only"
    ? "打开系统设置"
    : "去授权";
  calClearBtn.hidden = !st.cache_count;

  if (!st.enabled) {
    calLine.textContent = "日历未开启（默认关闭）";
    return;
  }
  if (!st.can_read) {
    calLine.textContent = "看不到日程，先授权";
    return;
  }
  const bits = [];
  if (st.next) {
    const t = new Date(st.next.start_at);
    bits.push(
      `下一个 ${t.toLocaleTimeString("zh-CN", { hour: "2-digit", minute: "2-digit" })} ${st.next.title}`
    );
  } else {
    bits.push("接下来没安排");
  }
  bits.push(`今天读 ${st.today_reads} 次 · 提醒 ${st.today_reminds} 次`);
  calLine.textContent = bits.join(" · ");
}

async function refreshCalendar() {
  if (!inTauri) {
    calLine.textContent = "浏览器预览模式：后端未连接";
    return;
  }
  try {
    renderCalendar(await invoke("calendar_status"));
  } catch {
    calLine.textContent = "日历状态读取失败";
  }
}

// macOS only: deep link into the calendar privacy pane. Apple gives no
// programmatic way to grant this — the user must flip it themselves.
function openCalendarSettings() {
  if (!inTauri) return;
  invoke("calendar_open_settings").catch(() =>
    toast("请到「系统设置 · 隐私与安全性 · 日历」里打开")
  );
}

calEnabled?.addEventListener("change", async () => {
  if (!inTauri) return;
  const on = calEnabled.checked;
  try {
    const st = await invoke("calendar_set_config", { enabled: on });
    renderCalendar(st);
    // Turning it on is the ONLY moment we ask: user-initiated prompts are
    // the ones people actually accept.
    if (on && !st.can_read) {
      renderCalendar(await invoke("calendar_request_access"));
    }
  } catch {
    toast("日历设置失败");
  }
});

calRemind?.addEventListener("change", async () => {
  if (!inTauri) return;
  try {
    renderCalendar(await invoke("calendar_set_config", { remind: calRemind.checked }));
  } catch {
    toast("提醒设置失败");
  }
});

calAccessBtn?.addEventListener("click", async () => {
  if (!inTauri) return;
  const st = await invoke("calendar_request_access").catch(() => null);
  if (!st) {
    openCalendarSettings();
    return;
  }
  renderCalendar(st);
  if (!st.can_read) openCalendarSettings();
});

calClearBtn?.addEventListener("click", async () => {
  if (!inTauri) return;
  try {
    renderCalendar(await invoke("calendar_clear_cache"));
    toast("日历缓存已清空");
  } catch {
    toast("清空失败");
  }
});

// A reminder arrives already de-duplicated and rate-limited on the Rust side.
// Hidden window → say nothing (it is not a notification centre).
function showReminder(p) {
  if (!p || document.visibilityState === "hidden") return;
  const bubble = addAnnouncement(
    `${p.title}，${p.minutes_left} 分钟后开始。`,
    "会前提醒"
  );
  const mute = document.createElement("button");
  mute.className = "ob-btn cal-mute";
  mute.textContent = "今天别再提醒";
  mute.addEventListener("click", async () => {
    try {
      await invoke("calendar_mute_today");
      toast("今天安静了");
    } catch {
      toast("操作失败");
    }
    mute.remove();
  });
  bubble.appendChild(mute);
  selectTab("chat");
  stick();
}

// ---------------------------------------------------------------- boot

setInterval(() => {
  frame ^= 1;
  renderFrame();
}, FRAME_INTERVAL_MS);

setInterval(refreshPerceptionStatus, 15000);

if (inTauri) {
  listen("face:state", (event) => setState(event.payload)).catch(() => {});
  invoke("face_current").then(setState).catch(() => {});

  // L5: a proactive message arrives from the Rust loop (already
  // quota-checked there — this file never decides to speak).
  listen("proactive:message", (event) => {
    addBubble(event.payload.text, "bot", { proactive: true, note: "它自己开口的" });
    selectTab("chat");
    messages.scrollTop = messages.scrollHeight;
    toast("阿盆说了句什么");
    invoke("face_preview", { state: "greeted" }).catch(() => {});
  }).catch(() => {});

  // Predict-verify loop: one guess per day from Rust (evidence-backed,
  // quota-checked there). Three verdict buttons; feedback writes back to
  // preference confidence on the Rust side.
  listen("prediction:asked", (event) => {
    addPrediction(event.payload.id, event.payload.guess);
    selectTab("chat");
    messages.scrollTop = messages.scrollHeight;
    toast("阿盆猜了一件事");
  }).catch(() => {});

  // Form-line growth (P1 item 2): stage unlocks + dormancy flips arrive as
  // growth:stage; announcements drain at the window-open breakpoint only.
  listen("growth:stage", () => {
    drainGrowthAnnouncements().catch(() => {});
  }).catch(() => {});
  // Relationship line (P2 item 1): same window-open breakpoint, same
  // exactly-once queue. It never pops up on its own.
  listen("relationship:stage", () => {
    drainRelationshipAnnouncements().catch(() => {});
  }).catch(() => {});
  // Calendar reminders (P2 item 2): already de-duplicated, rate-limited and
  // quiet-hour-checked on the Rust side. A hidden window stays silent.
  listen("calendar:remind", (event) => showReminder(event.payload)).catch(() => {});
  listen("window:shown", () => {
    drainGrowthAnnouncements().catch(() => {});
    drainRelationshipAnnouncements().catch(() => {});
    refreshCalendar();
    checkOnboarding(); // tray "重新做一次引导" re-opens the flow here
  }).catch(() => {});
  drainGrowthAnnouncements().catch(() => {});
  drainRelationshipAnnouncements().catch(() => {});
  checkOnboarding();

  relResetBtn?.addEventListener("click", async () => {
    // Two-step arm instead of a native confirm(): webview dialogs are not
    // guaranteed on every platform. Second click actually resets.
    if (relResetBtn.dataset.armed !== "1") {
      relResetBtn.dataset.armed = "1";
      relResetBtn.textContent = "再点一次";
      setTimeout(() => {
        relResetBtn.dataset.armed = "";
        relResetBtn.textContent = "重新认识";
      }, 3000);
      return;
    }
    relResetBtn.dataset.armed = "";
    relResetBtn.textContent = "重新认识";
    await invoke("relationship_reset").catch(() => {});
    await drainRelationshipAnnouncements().catch(() => {});
    toast("重新认识了。记忆和盆栽都没动。");
  });

  face.addEventListener("click", () => {
    poke();
    invoke("face_poke").catch(() => {});
    // Stage-flavoured poke copy (plan §3.7: every stage changes an interaction).
    toast(POKE_LINES[Math.min(growth?.stage ?? 0, POKE_LINES.length - 1)]);
  });

  loadPerceptionConfig();
  refreshPerceptionStatus();
} else {
  // Browser preview fallback: local demo cycle.
  const DEMO = ["calm", "focused", "calm", "thinking", "calm", "sleepy"];
  let i = 0;
  face.addEventListener("click", () => {
    poke();
    setState("greeted");
    setTimeout(() => setState(DEMO[i]), 2500);
  });
  setInterval(() => {
    i = (i + 1) % DEMO.length;
    setState(DEMO[i]);
  }, 6000);
  syncSubsState();
  // Preview-only demo of the prediction card (buttons won't reach Rust).
  const demoCard = addPrediction(0, "（预览示例）我猜你接下来大概要打开 VSCode——猜错了别笑我。");
  demoCard.querySelector(".predict-row").remove();
  demoCard.querySelector(".badge").textContent = "预览示例 · 预测卡片长这样";
  perceptionLine.textContent = "浏览器预览模式：后端未连接";
  relationshipSub.textContent = "浏览器预览模式";
  // Preview: walk the onboarding flow once so it can be inspected.
  obQuestions = OB_QUESTIONS_FALLBACK;
  onboarding.hidden = false;
  obShow("age");
}

// Preview chips: show the expression right away, then let the Rust state
// machine push the real state back over face:state.
stateButtons.addEventListener("click", (e) => {
  const btn = e.target.closest("button");
  if (!btn) return;
  setState(btn.dataset.state);
  if (inTauri) invoke("face_preview", { state: btn.dataset.state }).catch(() => {});
});

// ⌘1/2/3 jump to a tab
document.addEventListener("keydown", (e) => {
  if (!(e.metaKey || e.ctrlKey)) return;
  const map = { "1": "chat", "2": "preview", "3": "tools" };
  if (map[e.key]) {
    e.preventDefault();
    selectTab(map[e.key]);
  }
});

renderFrame();
syncButtons();
syncSubsState();
autosize();
// First paint: put the default (S0) form on screen. The real state from
// Rust replaces this via drainGrowthAnnouncements → applyGrowth.
applyStageToSvg(Number(svg.dataset.stage) || 0, false, 0, 1);

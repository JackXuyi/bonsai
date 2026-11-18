// L6 — 12-frame emotion face renderer.
//
// Ownership split (research report §8.2): the Rust core owns the face STATE
// (see src-tauri/src/face.rs) and broadcasts `face:state` events; this file
// only renders frames. 12 frames = 6 states × 2 frames, toggled by a 600 ms
// frame clock.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

const FACE_STATES = ["calm", "focused", "sleepy", "greeted", "displeased", "thinking"];

const CAPTIONS = {
  calm: "平静地在窗台上待着",
  focused: "在看你写代码，不打扰",
  sleepy: "深夜了，话变少",
  greeted: "嘿，你来啦",
  displeased: "刚才那句，我有意见",
  thinking: "让我想想…",
};

// Each state has exactly 2 frames: [eyeL rx,ry, eyeR rx,ry], mouth path, whole-face transform.
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
};

const FRAME_INTERVAL_MS = 600;

const face = document.getElementById("face");
const svg = face.querySelector("svg");
const eyeL = face.querySelector(".eye-l");
const eyeR = face.querySelector(".eye-r");
const mouth = face.querySelector(".mouth");
const caption = document.getElementById("caption");
const buttons = document.getElementById("states");

const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

let state = "calm";
let frame = 0;

function renderFrame() {
  const pose = POSES[state][frame];
  eyeL.setAttribute("rx", pose.eyes[0]);
  eyeL.setAttribute("ry", pose.eyes[1]);
  eyeR.setAttribute("rx", pose.eyes[2]);
  eyeR.setAttribute("ry", pose.eyes[3]);
  mouth.setAttribute("d", pose.mouth);
  svg.style.transform = pose.transform;
  caption.textContent = CAPTIONS[state];
}

function syncButtons() {
  for (const btn of buttons.querySelectorAll("button")) {
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

// Frame clock: alternate frame 0 / 1 forever.
setInterval(() => {
  frame ^= 1;
  renderFrame();
}, FRAME_INTERVAL_MS);

if (inTauri) {
  // Rust is the source of truth.
  listen("face:state", (event) => setState(event.payload)).catch(() => {});
  invoke("face_current").then(setState).catch(() => {});

  // Clicking the face = saying hi (transient "greeted", handled in Rust).
  face.addEventListener("click", () => {
    invoke("face_poke").catch(() => {});
  });

  // The strip is a preview tool: forces a transient state via Rust
  // (same hook L4 growth events and L5 disagreements will use later).
  buttons.addEventListener("click", (e) => {
    const btn = e.target.closest("button");
    if (btn) invoke("face_preview", { state: btn.dataset.state }).catch(() => {});
  });
} else {
  // Browser preview fallback (no Tauri backend): local demo cycle.
  const DEMO = ["calm", "focused", "calm", "thinking", "calm", "sleepy"];
  let i = 0;
  face.addEventListener("click", () => {
    setState("greeted");
    setTimeout(() => setState(DEMO[i]), 2500);
  });
  setInterval(() => {
    i = (i + 1) % DEMO.length;
    setState(DEMO[i]);
  }, 6000);
}

renderFrame();
syncButtons();

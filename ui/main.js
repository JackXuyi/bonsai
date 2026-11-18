// L6 — Presentation layer glue (scaffold).
// Face states: calm / focused / sleepy / greeted / displeased / thinking.
// Every state change is logged as an event into the L2 memory store.

const FACE_STATES = ["calm", "focused", "sleepy", "greeted", "displeased", "thinking"];

const CAPTIONS = {
  calm: "平静地在窗台上待着",
  focused: "在看你写代码，不打扰",
  sleepy: "深夜了，话变少",
  greeted: "嘿，你来啦",
  displeased: "刚才那句，我有意见",
  thinking: "让我想想…",
};

const face = document.getElementById("face");
const caption = document.getElementById("caption");
const buttons = document.getElementById("states");

let tauri = null;
if (window.__TAURI__) {
  tauri = window.__TAURI__.core ?? window.__TAURI__;
}

function setState(state) {
  if (!FACE_STATES.includes(state)) return;
  face.dataset.state = state;
  caption.textContent = CAPTIONS[state];
  for (const btn of buttons.querySelectorAll("button")) {
    btn.classList.toggle("active", btn.dataset.state === state);
  }
  // L2: log the state change as an event (local SQLite; never leaves machine).
  if (tauri?.invoke) {
    tauri.invoke("add_event", { kind: "face_state", content: state }).catch(() => {});
  }
}

buttons.addEventListener("click", (e) => {
  const btn = e.target.closest("button");
  if (btn) setState(btn.dataset.state);
});

// Load persona (L3) — the card is text, model-independent.
(async () => {
  if (!tauri?.invoke) return;
  try {
    const persona = await tauri.invoke("get_persona");
    document.getElementById("name").textContent = `${persona.name} · bonsai`;
  } catch {
    // running outside tauri (plain browser preview) — fine for scaffold
  }
})();

// Idle demo cycle so the face feels alive in the scaffold.
const DEMO = ["calm", "focused", "calm", "thinking", "calm", "sleepy"];
let i = 0;
setState("calm");
setInterval(() => {
  i = (i + 1) % DEMO.length;
  setState(DEMO[i]);
}, 6000);

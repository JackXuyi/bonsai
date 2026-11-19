// L6 — renderer for the face + the P0 UI wiring.
//
// Ownership split (research report §8.2): Rust owns state and policy —
// face state machine, dialogue pipeline, perception switches, memory,
// report. This file only renders and forwards user intent.
//
// Tabs: 对话 (dialogue_chat + proactive:message) · 表情 (frame preview)
//       · 工具 (daily report, consolidate, manual proactive, perception).

import { invoke, Channel } from "@tauri-apps/api/core";
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
};

const FRAME_INTERVAL_MS = 600;
const SESSION_ID = "default";

const $ = (id) => document.getElementById(id);

const face = $("face");
const svg = face.querySelector("svg");
const eyeL = face.querySelector(".eye-l");
const eyeR = face.querySelector(".eye-r");
const mouth = face.querySelector(".mouth");
const caption = $("caption");
const buttons = $("states");
const messages = $("messages");
const chatEmpty = $("chat-empty");
const composer = $("composer");
const input = $("input");
const sendBtn = $("send");
const output = $("output");
const perceptionLine = $("perception-line");
const reportDialog = $("report-dialog");
const reportBody = $("report-body");

const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

let state = "calm";
let frame = 0;
let chatting = false;

// ---------------------------------------------------------------- face

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

// ---------------------------------------------------------------- tabs

$("tabs").addEventListener("click", (e) => {
  const btn = e.target.closest("button");
  if (!btn) return;
  for (const b of $("tabs").querySelectorAll("button")) b.classList.toggle("active", b === btn);
  for (const tab of document.querySelectorAll(".tab")) {
    tab.classList.toggle("active", tab.id === `tab-${btn.dataset.tab}`);
  }
  if (btn.dataset.tab === "tools") refreshPerceptionStatus();
});

// ---------------------------------------------------------------- chat

function addBubble(text, who, opts = {}) {
  chatEmpty.hidden = true;
  const bubble = document.createElement("div");
  bubble.className = `bubble ${who}${opts.proactive ? " proactive" : ""}`;
  bubble.textContent = text;
  if (opts.note) {
    const badge = document.createElement("span");
    badge.className = "badge";
    badge.textContent = opts.note;
    bubble.appendChild(document.createElement("br"));
    bubble.appendChild(badge);
  }
  messages.appendChild(bubble);
  messages.scrollTop = messages.scrollHeight;
  return bubble;
}

function addTyping() {
  const el = document.createElement("div");
  el.className = "bubble bot typing";
  el.textContent = "…";
  messages.appendChild(el);
  messages.scrollTop = messages.scrollHeight;
  return el;
}

function noteFor(outcome) {
  const notes = [];
  if (!outcome.model_used) notes.push("本地模型未就绪");
  if (outcome.filter?.unconditional_affirmation) notes.push("反谄媚过滤：拦截了无条件肯定");
  if (outcome.filter?.hedged_disagreement) notes.push("反谄媚过滤：命中了缓冲式反对");
  return notes.join(" · ");
}

function appendNote(bubble, note) {
  if (!note) return;
  const badge = document.createElement("span");
  badge.className = "badge";
  badge.textContent = note;
  bubble.appendChild(document.createElement("br"));
  bubble.appendChild(badge);
}

async function sendMessage(text) {
  if (!text.trim()) return;
  addBubble(text, "user");
  if (!inTauri) {
    addBubble("（浏览器预览模式：Rust 后端未连接，发消息不会有人回。）", "bot");
    return;
  }
  if (chatting) return;
  chatting = true;
  sendBtn.disabled = true;
  const typing = addTyping();
  // Streaming: deltas arrive while the model is still generating; the
  // typing placeholder is swapped for a real bubble on the first chunk.
  let bubble = null;
  const onDelta = new Channel();
  onDelta.onmessage = (msg) => {
    if (!msg?.delta) return;
    if (!bubble) {
      typing.remove();
      bubble = addBubble("", "bot");
    }
    bubble.textContent += msg.delta;
    messages.scrollTop = messages.scrollHeight;
  };
  try {
    const outcome = await invoke("dialogue_chat_stream", {
      userText: text,
      sessionId: SESSION_ID,
      onDelta,
    });
    if (!bubble) {
      // No deltas at all (fallback line / dead model) — render whole reply.
      bubble = addBubble("", "bot");
    }
    bubble.textContent = outcome.reply;
    appendNote(bubble, noteFor(outcome));
    // Speaking counts as "being talked to": flash the greeted face.
    invoke("face_preview", { state: "greeted" }).catch(() => {});
  } catch (err) {
    if (typing.isConnected) typing.remove();
    if (bubble?.isConnected) bubble.remove();
    addBubble(`（出错了：${err}）`, "bot");
  } finally {
    chatting = false;
    sendBtn.disabled = false;
    input.focus();
  }
}

composer.addEventListener("submit", (e) => {
  e.preventDefault();
  const text = input.value;
  input.value = "";
  sendMessage(text);
});

// ---------------------------------------------------------------- tools

function showOutput(text) {
  output.hidden = false;
  output.textContent = text;
}

async function withBusy(btn, label, fn) {
  if (!inTauri) {
    showOutput("（浏览器预览模式：Rust 后端未连接。）");
    return;
  }
  const original = btn.textContent;
  btn.disabled = true;
  btn.textContent = "…";
  try {
    await fn();
  } catch (err) {
    showOutput(`操作失败：${err}`);
  } finally {
    btn.disabled = false;
    btn.textContent = original || label;
  }
}

// minimal markdown → HTML for the report (headings / lists / quote / text)
function renderMarkdown(md) {
  const esc = (s) =>
    s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
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
      out.push(`<h2>${esc(line.slice(3))}</h2>`);
    } else if (line.startsWith("# ")) {
      closeList();
      out.push(`<h1>${esc(line.slice(2))}</h1>`);
    } else if (line.startsWith("- ")) {
      if (!inList) {
        out.push("<ul>");
        inList = true;
      }
      out.push(`<li>${esc(line.slice(2))}</li>`);
    } else if (line.startsWith("> ")) {
      closeList();
      out.push(`<blockquote>${esc(line.slice(2))}</blockquote>`);
    } else if (line.trim() === "") {
      closeList();
    } else {
      closeList();
      out.push(`<p>${esc(line)}</p>`);
    }
  }
  closeList();
  return out.join("");
}

$("btn-report").addEventListener("click", (e) =>
  withBusy(e.currentTarget, "今日日报", async () => {
    const md = await invoke("daily_report", {});
    reportBody.innerHTML = renderMarkdown(md);
    reportDialog.showModal();
  })
);

$("report-close").addEventListener("click", () => reportDialog.close());

$("btn-consolidate").addEventListener("click", (e) =>
  withBusy(e.currentTarget, "整理记忆", async () => {
    const r = await invoke("consolidate");
    showOutput(
      [
        `整理完成（${r.trigger}）`,
        `窗口内事件 ${r.events_in_window} 条`,
        `抽取 ${r.extracted} · 合并 ${r.merged} · 矛盾消解 ${r.contradicted}`,
        `升格 ${r.promoted} · 删除 ${r.deleted} · 消费 ${r.consumed}`,
      ].join("\n")
    );
  })
);

$("btn-proactive").addEventListener("click", (e) =>
  withBusy(e.currentTarget, "主动说一句", async () => {
    const text = await invoke("dialogue_proactive_now");
    if (text) {
      addBubble(text, "bot", { proactive: true, note: "主动搭话" });
      showOutput("它开口了（这条不受每日配额限制，是你手动要的）。");
    } else {
      showOutput("它没说话——本地模型没起来。模型不在时它选择沉默，不编话。");
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

async function loadPerceptionConfig() {
  if (!inTauri) return;
  try {
    const cfg = await invoke("perception_get_config");
    percInputs.enabled.checked = !!cfg.enabled;
    percInputs.clipboard.checked = !!cfg.clipboard;
    percInputs.active_app.checked = !!cfg.active_app;
    percInputs.downloads.checked = !!cfg.downloads;
  } catch {
    /* perception state not ready yet — the poll below will retry */
  }
}

async function savePerceptionConfig() {
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
    showOutput(`感知设置保存失败：${err}`);
  }
  refreshPerceptionStatus();
}

for (const el of Object.values(percInputs)) {
  el.addEventListener("change", savePerceptionConfig);
}

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
    return;
  }
  try {
    const s = await invoke("perception_status");
    if (!s.enabled) {
      perceptionLine.textContent = "感知层未开启（默认关闭）";
      return;
    }
    const parts = [fmtIdle(s.idle_secs)];
    if (s.front_app) parts.push(`前台 ${s.front_app}`);
    perceptionLine.textContent = parts.join(" · ");
  } catch {
    perceptionLine.textContent = "感知状态读取失败";
  }
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
    invoke("face_preview", { state: "greeted" }).catch(() => {});
  }).catch(() => {});

  face.addEventListener("click", () => {
    invoke("face_poke").catch(() => {});
  });

  buttons.addEventListener("click", (e) => {
    const btn = e.target.closest("button");
    if (btn) invoke("face_preview", { state: btn.dataset.state }).catch(() => {});
  });

  loadPerceptionConfig();
  refreshPerceptionStatus();
} else {
  // Browser preview fallback: local demo cycle.
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
  perceptionLine.textContent = "浏览器预览模式：后端未连接";
}

renderFrame();
syncButtons();

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

const FACE_STATES = ["calm", "focused", "sleepy", "greeted", "displeased", "thinking"];

const CAPTIONS = {
  calm: "平静地在窗台上待着",
  focused: "在看你写代码，不打扰",
  sleepy: "深夜了，话变少",
  greeted: "嘿，你来啦",
  displeased: "刚才那句，我有意见",
  thinking: "让我想想…",
};

const MOODS = {
  calm: "平静",
  focused: "专注",
  sleepy: "犯困",
  greeted: "回应中",
  displeased: "有意见",
  thinking: "思考",
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

const app = $("app");
const face = $("face");
const svg = face.querySelector("svg");
const eyeL = face.querySelector(".eye-l");
const eyeR = face.querySelector(".eye-r");
const mouth = face.querySelector(".mouth");
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
  if (caption.textContent !== CAPTIONS[state]) caption.textContent = CAPTIONS[state];
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
  if (name === "tools") refreshPerceptionStatus();
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

function appendNote(bubble, note) {
  if (!note) return;
  const badge = document.createElement("span");
  badge.className = "badge";
  badge.textContent = note;
  bubble.appendChild(document.createElement("br"));
  bubble.appendChild(badge);
}

function noteFor(outcome) {
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

  face.addEventListener("click", () => {
    poke();
    invoke("face_poke").catch(() => {});
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

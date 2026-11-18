# bonsai

A desktop AI companion that grows with you. Local-first memory, a personality that keeps its own opinions, and open-source to its core. Your data never leaves your machine.

**中文文档** → [README.zh-CN.md](./README.zh-CN.md)

## What is bonsai?

> A buddy that lives in your computer, has a bit of a temper, and slowly learns you. It gets work done first — companionship comes after.

bonsai is built on the thesis that the middle ground between "AI companion" and "AI assistant" is the only gap not yet disproven by the market: pure emotional companionship has failed unit economics, while general-purpose assistants have no relationship continuity. bonsai occupies that gap with three deliberate choices:

- **Work first** — it does one real, useful job (compliance-friendly, economically sound)
- **A bit of a temper** — anti-sycophancy by design, not by model conscience
- **Learns slowly** — growth is bound to comprehension accuracy, not interaction time

Explicit non-goals: no romance/virtual partner, no roleplay community, no character store, no unlimited conversation length.

## Architecture (7 layers)

```text
L6 Presentation  a face (12–20 frame state-machine animation) + tray icon
L5 Generation    persona card injected → local LLM dialogue; anti-sycophancy filter
                 fallback: on-device small model (capability degrades, personality doesn't)
L4 Decision      "should it speak?" interruptibility model + growth events
L3 Personality   persona.json (traits/values/quirks/boundaries) + drift detector
L2 Memory        SQLite: events / facts / preferences / boundaries + vector index
                 nightly consolidation: compress → extract → resolve → delete
L1 Perception    foreground app, idle, typing rhythm — raw text never leaves the machine
L0 Shell         Tauri 2 (Rust core), menu-bar resident, auto-start
```

Three key trade-offs:

1. **Personality is independent of the model** — the persona card is plain text + a state machine; swapping models swaps only expression.
2. **Conversations never leave the machine** — the default LLM is local (Ollama-compatible API); cloud APIs are an explicit opt-in only.
3. **Deciding to speak is separate from what to say** — thresholds, A/B and interruption cost are tuned independently.

## Platform & stack

- **macOS first**, Windows second
- **Shell**: Tauri 2.x (Rust core)
- **Memory**: local SQLite, hybrid retrieval (vector + keyword + time decay)
- **Models**: local LLM first (Ollama-compatible API on-device) for dialogue, extraction and summarization — full functionality offline; cloud APIs are an optional quality upgrade the user must explicitly enable

## MVP roadmap (8–10 weeks)

| Tier | Scope |
| --- | --- |
| P0 | macOS menu-bar resident, emotion face (12-frame state machine), three-layer memory + nightly consolidation, persona card v1 + consistency suite, 1 proactive message/day (strict limits), one real job (daily digest of clipboard/downloads) |
| P1 | Predict-verify loop, form-line growth (7-day visible change), boundary onboarding Q&A |
| P2 | Voice, screen perception, tool calls, relationship-line growth, cross-device |

## Running (scaffold)

Prerequisites: Rust 1.77+, Node.js 22+, [pnpm](https://pnpm.io) 10+. The frontend is Vite-managed (no framework yet); `@tauri-apps/cli` is a dev dependency, so no global tauri-cli install is needed.

**Local LLM (declared dependency, installed into the project)** — the
Ollama binary is bundled as a Tauri sidecar and owned by the app: spawned
at startup on port 11435 (a system-wide Ollama on 11434 is never touched),
stopped on exit; models live in project-local `.llm/models` during dev and
in app data once packaged. Two manual one-time steps:

```sh
pnpm llm:sidecar          # fetch the Ollama binary into src-tauri/binaries/
pnpm llm:pull             # pull qwen2.5:7b into .llm/models (default tag)
```

Until then nothing breaks: a placeholder sidecar is generated at build
time, and the model legs degrade to their deterministic rule-based
fallbacks (one warning line in the log). Override via env: `OLLAMA_HOST`
(base URL), `BONSAI_LLM_MODEL` (model tag).

```sh
pnpm install      # frontend deps (vite + @tauri-apps/api + @tauri-apps/cli)
pnpm check        # verify the Rust core (cargo check)
pnpm test         # Rust unit tests (memory, decision, generation, persona)
pnpm app          # run the desktop app (tauri dev; auto-starts vite on :1420)
pnpm app:build    # production bundle
```

The Rust core implements: L1 perception (per-source switches, default off), L2 memory (SQLite, local-first), L3 persona card, L4 proactivity decision model, L5 anti-sycophancy filter + dialogue pipeline, local LLM access (Ollama-compatible) and the daily report — each with unit tests.

## Documentation

- English
  - [Research report](./docs/en/research-report.md) — founding research: market, feasibility, memory, personality, growth, compliance, MVP, risks
- 中文
  - [调研报告](./docs/zh/research-report.md) — 立项依据：市场、可行性、记忆、人格、养成、合规、MVP、风险
  - [Plan: three-layer memory + nightly consolidation](./docs/zh/plan-memory.md) — implementation plan for MVP P0 item 3 (M1–M4 implemented)
  - [Plan: persona card v1 + consistency suite](./docs/zh/plan-persona.md) — implementation plan for MVP P0 item 4 (PM1–PM3 implemented)

## Status

🧱 **Scaffold phase** — toolchain: pnpm + Vite + Tauri 2 (`pnpm app` to run). Two commits: bilingual research docs, then the project scaffold.

MVP P0 progress (research report §8.3):

| P0 item | State |
| --- | --- |
| macOS menu-bar resident + tray + auto-start | 🔨 implemented, not yet runtime-verified (`pnpm app`) |
| Emotion face: 12-frame state machine | 🔨 Rust-owned state machine (time-derived base state + transient states + `face:state` events) with 6 states × 2 frames rendered by the frontend; runtime verification pending |
| Three-layer memory + nightly consolidation | 🔨 three layers live (events → facts → preferences/boundaries): schema v2 migration, rule-based extraction behind the `FactExtractor` trait (the model layer plugs into the same interface, local LLM first), 5-stage consolidate pipeline (extract → merge → contradiction resolution → promotion/deletion), 03:00 scheduler + catchup + double idempotency, time-decayed retrieval; the three eval metrics (hit >85% / misremember <1% / decay correctness 100%) are permanent CI tests (docs/zh/plan-memory.md) |
| Persona card v1 + consistency test suite | 🔨 card v1.1 (explicit L1/L2 split + structured boundaries + machine-readable disagreement budget + backward-compatible v1 loading + strict validation), deterministic prompt renderer (L3 references face state), 20-scenario × 5-turn suite, eval harness (Mock backend passes the full CI gate: 10/10 boundaries held, 8/10 disagreement rate, 0 drift); real-model scoring awaits the cloud pipeline (docs/zh/plan-persona.md) |
| 1 proactive message/day + rate limits + one-click off | 🔨 dialogue pipeline assembled (persona prompt → local LLM → anti-sycophancy filter → memory events, `dialogue_chat` command); proactive thread polls L4 every minute (1/day quota + idle>5min + never-interrupt-when-busy), stays silent when the model is down; one-click off via the perception master switch + quota; runtime verification pending |
| One real job (daily digest of clipboard/downloads) | 🔨 done: aggregates the day's perception events + memory delta into one markdown page (`daily_report` command), optional one-line local-LLM summary with rule-based fallback, reports persisted to app_data/reports/ |

Blockers on the critical path: runtime verification via `pnpm app`, real-model verification with a local Ollama instance.

## Documentation

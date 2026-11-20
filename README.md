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
pnpm llm:sidecar          # fetch the Ollama runtime (CLI + llama-server) into src-tauri/binaries/
pnpm llm:pull             # pull qwen3.5:2b into .llm/models (default tag)
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
  - [Plan: predict-verify loop](./docs/zh/plan-predict.md) — implementation plan for MVP P1 item 1 (G1–G4 implemented)
  - [Plan: form-line growth](./docs/zh/plan-morph.md) — implementation plan for MVP P1 item 2 (G1–G4 implemented)
  - [Plan: relationship-line growth](./docs/zh/plan-relationship.md) — implementation plan for MVP P2 item 1 (Q1–Q4 implemented)
  - [Plan: calendar / mail tool calls](./docs/zh/plan-calendar.md) — implementation plan for MVP P2 item 2 (T0–T4 implemented; mail T5 not done)
  - [Plan: MCP calls + Skill support](./docs/zh/plan-agent-mcp-skill.md) — general agent capabilities (S1–S6 implemented; specs under [docs/zh/specs/agent](./docs/zh/specs/agent/README.md))

## Status

🧱 **Usable phase** — toolchain: pnpm + Next.js (App Router, `frontend/`) + Tauri 2 (`pnpm app` to run). All six P0/P1 features plus the P2 relationship line, calendar tools and the MCP/Skill pipeline are implemented in code; the sidebar exposes five pages (chat / face / memory / tools / settings).

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

MVP P1 progress:

| P1 item | Status |
| --- | --- |
| Predict-verify loop | 🔨 implemented: at most one explainable guess per day (habit direct / 14-day rhythm extrapolation / continuity), three-tier feedback (hit / miss / shut_up) written back into preference confidence, 3-day back-off on "shut_up"; mutually exclusive with the proactive channel, never interrupts when busy; metric gates (accuracy ≥50%, back-off rate <10%) are permanent CI tests (docs/zh/plan-predict.md) |
| Form-line growth (7-day visible change) | 🔨 implemented: four stages over seven days (破土 day 0–1 / 抽枝 day 2 / 成形 day 4 / 成景 day 6), time-driven with no check-ins and no scores; schema v4 growth tables (birth anchor backfilled from the earliest event, monotonic stage, idempotent daily tick); daily micro-variations (deeper leaf tint + deterministic extra leaves + stage self-narration); dormancy instead of death (7 silent days → wilted resting form, first activity wakes it); unlock announcements queue until the user opens the window and fire exactly once (`notified_at` idempotency, no interruption budget spent); S3 unlocks the `proud` face and stage-flavoured poke copy; three metrics (unlock punctuality / announcement idempotency / dormancy correctness) are permanent CI tests (docs/zh/plan-morph.md) |
| Boundary onboarding Q&A | 🔨 Implemented: modal first-run flow (age gate → crisis resource card → five questions → done; everything but age is skippable); schema v5 settings + onboarding_answers audit table (skips recorded too); q1/q2/q4 split mechanically and stored verbatim as boundaries (prompt injection already picks them up; resubmission is idempotent), q3/q5 go to preferences, not boundaries; "never record" items are blocked at the memory-extraction door (raw event kept, never becomes a fact); rule-level crisis intercept (works with the model down) + a second minor gate at the dialogue commands; tray "redo onboarding" keeps answers; tools page lists and deletes boundaries; three metrics (transcription fidelity / idempotency / intercept correctness) are standing CI tests (see docs/zh/plan-onboarding.md) |

MVP P2 progress:

| P2 item | Status |
| --- | --- |
| Relationship-line growth (day 8–30, driven by understanding accuracy) | 🔨 Implemented: three stages (拘谨 → 熟络 at day ≥ 9 with ≥ 12 evidence points → 会顶嘴 at day ≥ 22 with ≥ 30); both conditions required — days only open the window, they never carry progress. Evidence is recomputed daily from predictions + events (presence / hits / un-waved push-backs +1 each, a wrong guess −1 and never compounding); the score never leaves the process and has no UI. "Shut up" never subtracts points, it only pauses promotion (back-off > 10%). Seven silent days → rusty tone without rewinding the stage. The prompt gets one of four relationship modes; at R2 the banned hedge openers become a hard rule. Anti-sycophancy hits are finally persisted as disagreement / disagreement_hedged events. A once-ever "first time it didn't go along with you" moment inside the 7-day window after R2. Tools page shows one line + a relationship-only reset. Four metrics are permanent CI tests (see docs/zh/plan-relationship.md) |
| Calendar / mail tool calls | 🔨 Implemented for calendar only (mail not done): schema v7 `calendar_cache` (5-minute TTL, titles live here and nowhere else, `reminded_at` idempotency) + `tool_calls` audit (actions and counts, never content); EventKit is called inside this process (TCC attributes permission to the responsible process — a helper would own a second identity), the prompt is dispatched to the main thread with a 60s timeout, and **only `fullAccess` counts** (`writeOnly` cannot read); denials return `calendar_denied` and friends, never an empty list pretending "no meetings today"; rule-based intent routing (time word AND schedule noun, 20 positive / 15 negative CI cases) sits after the crisis intercept and before the model, answered by deterministic templates that work with the model down; pre-meeting reminders are a second L4 lane (−10±2 min window, ≤3/day, ≥10 min apart, silent 22:00–08:00 and during meetings, skips all-day/declined/canceled, "don't remind me today" mutes the rest of the day); `in_meeting` now feeds `should_speak` (two or more humans required, so a solo focus block never silences it); the daily report gains a "tomorrow" section with three distinct states (no calendar / not authorised / actual plans); tools page card shows "read N times today"; mail (AppleScript) is deferred to T5 (see docs/zh/plan-calendar.md §3.9/§11) |
| MCP tools / Skill packs / call audit | 🔨 Implemented (P3-4 pulled forward, see docs/zh/plan-agent-mcp-skill.md): schema v8 `mcp_servers`; dual MCP transport (stdio child process + remote Streamable HTTP) behind the official rmcp SDK, with the async boundary confined to `mcp.rs` and a sync facade outward (connect 10s / list+call 60s hard timeouts; unreachable always surfaces `mcp_unreachable`, never a fake empty result; `shutdown_all` on exit prevents orphan processes); new servers always start disabled (RK-04); headers/env are plaintext only inside the local SQLite file and masked on every read path; **three-stage progressive disclosure**: the system prompt carries names only (≤20) → the model names candidates (whitelisted, ≤5) → full descriptions/input schemas are injected → the tool runs → the answer is written from a result summary, with **zero extra rounds when nothing is selected** (indistinguishable from the previous behaviour); Skills are prompt-only (body ≤8KB, references ≤4KB×5, injection blacklist rejects the pack), at most two full bodies injected per turn; the approval hook (`ApprovalGate`) defaults to `auto`, and `ask` pops a confirm card where 30 seconds of silence counts as a denial; `tool_calls` audit rows carry a masked params summary, result_status, a prompt_trace reference and a remote flag, readable from the tools page, with a 🔧 badge on the chat bubble; the environment segment (time / weekday / timezone / language / platform) is system-injected and declared immutable; covered by 281 Rust tests + 70 frontend tests |
| Voice (occasional utterances) | ⬜ not started |
| Screen perception | ⬜ not started |
| Cross-device / data portability | ⬜ not started |

## Documentation

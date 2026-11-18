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
L5 Generation    persona card injected → cloud dialogue; anti-sycophancy filter
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
2. **Raw perception data never leaves the machine** — the cloud receives only structured labels.
3. **Deciding to speak is separate from what to say** — thresholds, A/B and interruption cost are tuned independently.

## Platform & stack

- **macOS first**, Windows second
- **Shell**: Tauri 2.x (Rust core)
- **Memory**: local SQLite, hybrid retrieval (vector + keyword + time decay)
- **Models**: hybrid edge-cloud routing; edge for perception/extraction only, cloud for dialogue; on-device fallback in privacy mode

## MVP roadmap (8–10 weeks)

| Tier | Scope |
| --- | --- |
| P0 | macOS menu-bar resident, emotion face (12-frame state machine), three-layer memory + nightly consolidation, persona card v1 + consistency suite, 1 proactive message/day (strict limits), one real job (daily digest of clipboard/downloads) |
| P1 | Predict-verify loop, form-line growth (7-day visible change), boundary onboarding Q&A |
| P2 | Voice, screen perception, tool calls, relationship-line growth, cross-device |

## Running (scaffold)

Prerequisites: Rust 1.77+, Node.js 22+ (frontend is plain static assets — no bundler yet).

```sh
cd src-tauri
cargo check     # verify the Rust core
cargo test      # unit tests (memory, decision, generation, persona)
cargo tauri dev # run the desktop app (requires tauri-cli: cargo install tauri-cli)
```

The Rust core implements: L2 memory (SQLite, local-first), L3 persona card, L4 proactivity decision model, L5 anti-sycophancy filter — each with unit tests. L1 perception and the cloud dialogue pipeline are not implemented yet.

## Documentation

- English
  - [Research report](./docs/en/research-report.md) — founding research: market, feasibility, memory, personality, growth, compliance, MVP, risks
- 中文
  - [调研报告](./docs/zh/research-report.md) — 立项依据：市场、可行性、记忆、人格、养成、合规、MVP、风险

## Status

📋 Project initialization phase — research compiled into bilingual docs; implementation not started.

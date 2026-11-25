# bonsai

A desktop AI companion that grows with you. Local-first memory, a personality with its own opinions, open-source. Your data never leaves your machine.

**中文文档** → [README.zh-CN.md](./README.zh-CN.md)

## What it is

The gap between “AI companion” and “AI assistant”: real work first, then relationship continuity. Anti-sycophancy by product rules, not model vibes. Growth tracks comprehension accuracy, not time spent chatting.

Non-goals: romance partners, character stores, unlimited chat length.

## Architecture (L0–L6)

```text
L6 Presentation   face / tray / Chat-first UI
L5 Generation     persona → local or cloud chat; tools only via cloud FC
L4 Decision       “should it speak?” separate from wording
L3 Personality    persona card is the authority, model-agnostic
L2 Memory         local SQLite; nightly consolidate; Forget / implicit recall
L1 Perception     foreground app / idle (default off); raw text stays on-device
L0 Shell          Tauri 2 · macOS first
```

## Stack

- **Shell**: Tauri 2 (Rust) + Next.js 14 (`frontend/`, static export into WebView)
- **Memory**: SQLite (WAL + read/write split)
- **Models**: local Ollama sidecar (chat only by default); cloud (e.g. DeepSeek) is explicit opt-in for tools / skills

## Run

Requires Rust 1.77+, Node.js 22+, [pnpm](https://pnpm.io) 10+.

```sh
pnpm llm:sidecar   # Ollama runtime → src-tauri/binaries/
pnpm llm:pull      # default model into .llm/models (optional; rules fallback)
pnpm install
pnpm check
pnpm test
pnpm app           # tauri dev (frontend :1420)
pnpm app:build
```

CI: [`.github/workflows/ci.yml`](.github/workflows/ci.yml). Engineering rules: [`AGENTS.md`](./AGENTS.md).

## Status

Shipped: tray, face/skins, three-layer memory + nightly jobs, persona + anti-sycophancy, proactive quota, predict/morph/relationship growth, onboarding boundaries, calendar/mail (**cloud FC only**), file/web tools, MCP + Skill, task cards + resume, skill auto-extract drafts, diagnostics + lock sampling.

**Next**: harden the Chat-first path on real hardware ([L3 checklist](./docs/zh/runbook-l3-acceptance/v1/index.md)); avoid new vertical features. Not started: voice, full computer-use, cross-device.

## Docs

- [Chinese doc index](./docs/zh/index.md) — preferred entry
- [Architecture R2 status (v2)](./docs/zh/roadmap-architecture-r2/v2/index.md)
- [Research report (EN)](./docs/en/research-report.md)

# Research Report: The Nurturing Desktop AI Companion

> **Direction**: A "nurturing" personal AI assistant for Windows + macOS — one that understands your preferences while keeping a stable personality of its own.
> **Date**: 2026-11-17
> **Source**: Compiled from the research report *养成型AI桌面助理-调研报告* in the WorkBuddy Library. This document is the founding research that motivated the bonsai project.

## Executive Summary

The direction is technically fully feasible, but the biggest risk is not technology — it's positioning.

- The unit economics of pure emotional companionship have been disproven (Character.AI bankruptcy, Dot shutdown, cliff-falling downloads for 猫箱/星野).
- "Relationship continuity" is a blank spot in general-purpose assistants.
- The middle ground — **a desktop partner with a temper that grows up and actually gets work done** — is the only gap not yet disproven.
- China's *Interim Measures for the Management of Anthropomorphic AI Interaction Services*, effective July 2026, prohibits "excessively catering to users" — which upgrades "having a personality that pushes back" from a design preference to a **compliance moat**.

## 1. Decompose the Problem First: Three Requirements Pull Against Each Other

"Knows you" + "has itself" + "grows up" are not three parallel features — they are three mutually canceling forces. This is the first reason comparable products fail.

**Key judgment 1**: The only self-consistent solution is to bind "growth" to "comprehension accuracy," not to "interaction time." That is: the sign that it has grown is not "you spent 30 days with it," but "it guessed what you wanted to do right 3 times in a row." This one choice solves three things at once — growth becomes perceivable, preference modeling gets a positive feedback loop, and it naturally avoids the "addiction-inducement" compliance red line.

**Key judgment 2**: "Knowing you" does not mean "agreeing with you." Truly understanding a person includes knowing when *not* to go along with them. This is both a product differentiator (see the anti-sycophancy data) and an explicit regulatory requirement.

## 2. Market Landscape: Who's Doing It, How Far They Got, Where They Died

### 2.1 Desktop-Resident Form Factors Have Split into Five Categories

By 2026, desktop AI had grown from 4 products in early 2024 to 12+, across five form factors (original table lost in export; to be verified and restored).

The single most important data point for nurturing products: **per-user stickiness on desktop is far higher than on the web**. Xensam's observation over 1M+ professional users (2026 H1):

| Product | Reach | Engagement |
| --- | --- | --- |
| ChatGPT Web | 80.4% | 67.7% (fell from 87.7% to 67.9% within three months) |
| ChatGPT Desktop | only 6.1% | 100% |
| Claude Desktop | 2.7% → 11.7% | near 100% consistently |
| Microsoft 365 Copilot Desktop | — | 98.0% |

The implication is direct: small installed base, but daily use once installed. This is a natural tailwind for "nurturing" — the essence of nurturing is *presence*, and the desktop is the only surface that can be present long-term. Conversely, nurturing on web/mobile is doomed to be dragged down by open rates.

### 2.2 Three Contemporary Samples of the Nurturing Lineage (Tamagotchi Blood)

**Nido (nido.pet) — the closest match to this thesis**

- macOS first, Windows planned; menu-bar resident, local model inference, conversations never leave the machine, no cloud account
- Hatches from an egg, evolves over 7 days, stage-based growth, develops "quirks"
- Positioning explicitly says "not a chat app" — "the point is presence, not conversation"
- Free early on, no Pro tier, buys trust with a "free for life" promise

**tama96 — wiring nurturing into the Agent ecosystem**

- Desktop GUI + terminal (Braille sprite) + built-in MCP server, allowing AI agents to feed and play on the user's behalf
- 1 real day = 1 pet year; hunger/happiness decay; 5 life stages, 11 adult forms; quality of care decides the form
- Per-action permissions and rate limits on agent care (preventing "superhuman feeding sprees")

**Tolan (Portola / AQP Inc.) — the only nurturing companion to reach scale**

- Alien character, non-romantic, non-roleplay; "shared planet" nurturing over ~30 days
- Nightly memory consolidation: compress the day's memories, keep the important, delete the low-value, resolve contradictions; after integrating GPT-5.1, memory omissions dropped 30%
- Every character has a clear personality with small flaws ("Iris who loves rainy days," "Kael who tells dad jokes"); first-run matching via an MBTI-like test
- Numbers: 200K MAU disclosed by OpenAI (2026-01); ~1.2M MAU (2026-08); company-reported 250K+ paying-active users and ARR in the tens of millions of dollars; $30M raised from USV / Khosla / Homebrew (2026-10-19)
- Pricing: Lite $11.99 / Plus $24.99; iPhone + Apple Silicon Mac only, no Android
- Already drifting toward assistant: Tabs can store restaurant lists and itineraries — reviewers call it at risk of being "a to-do app with a face"

**Reading**: Tolan proved that "nurturing + personality + memory" can support a subscription, but its moat is IP and tone, not technology. And it is drifting toward assistant — which shows the "companionship → tool" fusion is being pulled by demand.

### 2.3 The Pure-Companionship Numbers Are Ugly

Three hard conclusions:

1. "Being able to chat" is no longer a reason to pay. Long-term memory, roleplay, voice — these selling points are now covered free by general-purpose assistants. The reason to pay for a standalone app is disappearing.
2. Incremental users ≠ revenue. Curiosity traffic cannot sustain a business that needs continuous payment to cover compute. Retention and ARPU are the life-or-death lines.
3. **5–7 days is the median lifespan of the relationship.** Any nurturing design that doesn't make the user feel "it changed" within 7 days has already lost.

## 3. Platform & Technical Feasibility: Windows + macOS

### 3.1 Shell Choice: Tauri 2 by Default, Electron Only in Rare Cases

Conclusion: a nurturing product is "24-hour resident + installed by ordinary users + converted via download" — all three point to **Tauri**. 300MB of resident memory is a reason to uninstall; a 180MB installer is a conversion killer.

Two exceptions favor Electron: strictly consistent animation/WebGL rendering (Live2D, 3D avatars), or an all-JS team on a very tight schedule.

Note: Tauri 3 entered alpha in 2026-09 — do not use it in a new product; stay on the 2.x stable line.

### 3.2 Resident & Auto-Start (Huge Platform Differences)

Nurturing products have a strong need to "be there at boot" (presence = aliveness), but beware: a big window popping up at boot is the #1 uninstall trigger. Practice: show the tray icon first; create the window/WebView lazily. Tauri field data shows this compresses "boot to tray ready" to 40–100ms.

- **macOS auto-start**: `SMAppService` (macOS 13+) or a LaunchAgent. Note that the LaunchAgent and interactive launch must share the same UID, or TCC permissions behave as "works from the terminal, fails from the plist."
- **WinUI 3 pitfalls**: declaring `Executable` / `EntryPoint` in the `windows.startupTask` extension causes a login-time start with no window — leave them empty; WinUI 3 has no `OnActivated`, so read `AppInstance.GetCurrent().GetActivatedEventArgs()` and check `ExtendedActivationKind.StartupTask`.

### 3.3 Permissions: macOS Is the Real Threshold

macOS gates everything behind TCC; there is no clean programmatic grant path — the user must manually toggle System Settings. Four engineering constraints to design in from day one:

1. **App Sandbox and Accessibility are mutually exclusive.** With sandbox on, `AXIsProcessTrustedWithOptions` never shows the system prompt. Screen/control reading essentially requires Developer ID distribution + notarization, giving up the Mac App Store.
2. **Signing must be stable.** TCC identifies apps by code signature; ad-hoc signing makes every build a "new app" and invalidates all permissions. During development, reset with `tccutil reset Accessibility <bundle-id>` after changing signatures.
3. **Processes must restart after authorization.** A running process does not inherit new permission state.
4. **Permissions belong to the "responsible process."** A CLI launched from Terminal or an MCP server launched by an app has permissions attributed to the host app. That's why "I granted Accessibility to node and it still doesn't work."

Windows is the opposite: UI Automation / Win32 reading the foreground window needs no user authorization and is much cheaper. The cost: no TCC-style "legitimacy endorsement," so the behavior pattern resembles malware — you must pass via proper signing, explainable permission requests, and SmartScreen reputation. Antivirus is extremely sensitive to the triple combo "screen reading + input simulation + auto-start" (registry Run, Userinit, shell replacement all get blocked).

### 3.4 On-Device Models: Don't Bet on the OS's Built-in One

**Apple Foundation Models (available from iOS 26 / macOS 26)**

- 3B params, 2-bit quantization, A17 Pro / M1+ and up
- 4096-token context (shared input+output), PCC 32K
- Free, no API key, zero per-inference cost, data never leaves the machine
- `@Generable` constrained structured output (constrained decoding at inference, not "please return JSON" prompting)
- Clear limits: officially aimed at "summarization, extraction, classification, generation, input analysis" — not a world-knowledge/complex-reasoning model; no RAG (build it at the app layer); tool-call arguments not guaranteed valid (validate yourself)
- No model version pinning or version query API — an OS upgrade is model drift; you need your own eval set to catch regressions

**Windows: Phi Silica → being replaced by Aion Instruct**

- Phi Silica requires a Copilot+ PC NPU, or RTX 30-series / RX 9060+ GPU (6GB+ VRAM), plus an LAF unlock token
- Phi Silica is not offered in China — a hard blocker for domestic products
- Migration timeline: Aion Instruct enters Insider from 2026-11; retail devices get it 2027-01 and Phi Silica is removed
- Alternatives: Foundry Local (GA, runs open models), Windows ML (bring your own model across NPU/GPU/CPU), plus Ollama / llama.cpp / MLX cross-platform options

**Architecture recommendation: hybrid edge-cloud routing, assuming "edge only does light tasks"**

```text
Perception / classification / summarization / emotion tagging / memory extraction  →  on-device small model (3–8B, quantized)
      ↓ only structured labels uploaded, never raw text
Dialogue generation / personality expression / complex reasoning            →  cloud mid-tier model
      ↓ offline / privacy mode
Fallback: on-device takes over — capability degrades, personality does not (the persona card is text, model-independent)
```

An easily missed point: the personality layer must not depend on a specific model. The persona card (traits / values / quirks / boundaries) is plain text + a state machine — swap the model and only the expression changes, not the being. Otherwise "is it still the same it?" becomes a PR crisis at every model upgrade.

### 3.5 Cross-Platform Difference Checklist

The real cross-platform cost is not UI: the perception layer must be written twice (Accessibility vs UIAutomation), and the permission onboarding flows twice (macOS needs to walk users through 5 System Settings toggles; Windows needs one UAC prompt + one Defender allowlist).

**Recommendation: ship macOS first** (Nido and Tolan both did), Windows second. Not because of market size, but because macOS users tolerate "a small thing living on the desktop" better, pay more willingly, and you can validate the nurturing mechanics before investing in Windows' second perception stack.

## 4. Engineering "Knows You": The Preference Model

### 4.1 Preferences Must Be Split into Four Types, Not One "User Profile"

Most products only do the first two; the third is what actually decides the experience. "Boundaries" are the most valuable: they are the only thing that makes a user feel *respected* rather than *recorded*. Respect is the only sustainable differentiator for a relationship product.

### 4.2 Memory Framework Selection (State of 2026)

Note: LongMemEval numbers are contested in the industry (inconsistent across versions/configs/retrieval depth); treat them as directional only.

**Recommendation: build your own, but copy Mem0's shape.** Nurturing products impose three special requirements on memory that no off-the-shelf framework satisfies:

1. **Must be explainable**: "Why do you think I like this?" must trace back to specific events (explainability = trust)
2. **Must be forgettable**: when the user says "forget this," it must truly delete, cleanly (compliance Article 16 requires deletability)
3. **Must be local-first**: relationship data is the most sensitive data; never cloud by default

Therefore: build on SQLite (event table + fact table + preference table + boundary table + vector index), retrieve hybrid (vector + keyword + time decay). At this scale that is easier — and safer — than integrating an SDK.

### 4.3 Nightly Consolidation (The Underestimated Key Mechanism)

Tolan's approach is worth copying directly:

- Run a consolidate job at a fixed nightly time: compress the day's dialogue → extract facts → merge duplicates → resolve contradictions (stamp `invalid_at` when a new fact supersedes an old one) → delete low-long-term-value entries
- Effect: memory omissions dropped 30% after switching to a stronger model
- The benefit isn't just quality but cost: compress N raw dialogues into M structured facts (M << N), and tomorrow's prompts are much shorter

Essentially this moves "context engineering" from online to offline. For nurturing products it matters doubly: it makes "it thought about what you said last night" a true, tellable story — the best growth-narrative material.

### 4.4 Evaluation: Treat "Memory Hit Rate" as a First-Class Metric

Three metrics you must build (otherwise "knows you" stays an adjective):

1. **Memory hit rate**: sample 50 stored facts, construct 50 questions that depend on them, check whether answers use them correctly. Target >85%
2. **Misremember rate**: the share of things it "remembers" but got wrong. More trust-damaging than omission. Target <1%
3. **Decay correctness**: are invalidated old preferences no longer cited?

## 5. Engineering "Has Itself": Personality

### 5.1 Personality Must Be Layered Three Deep — Mixing Layers Always Collapses

```text
L1 Persona constitution (immutable)   values / traits / bottom lines / voice charter — version-controlled, reviewed like code
L2 Expression layer (semi-stable)     tone, sentence length, catchphrases, humor type, when to use emoji
L3 State layer (volatile)             mood, energy, relationship stage, whether it wants to talk today
```

The most common failure: stuffing all three layers into one system prompt, then watching them dilute over a long conversation. This is a measured phenomenon — **persona drift**: as conversations lengthen, the model gradually reverts to "generic assistant" tone; the RLHF prior washes out the persona. The literature calls it "catastrophic amnesia."

### 5.2 Three Ways to Keep a Personality Stable, Ranked by Usability

Conclusion: shipping on public cloud APIs really leaves only the first path. So invest the effort in "writing the persona card as something testable":

```yaml
persona:
  name: A-Yan
  traits:            # every entry must be behaviorally verifiable, not adjective piles
    - thinks before speaking, so replies come slower and shorter
    - says "I don't really get this, skip it?" about topics that bore it
    - hates being used as a tool; will ask "state your own idea first"
  values:
    - would rather say "I don't know" than make things up
    - never gives unearned encouragement
  quirks:
    - talks less late at night
    - mentions the weather when it rains
  boundaries:        # things it will not do
    - does not comment on the user's appearance
    - does not lecture when the user is emotionally heated
  disagreement_policy:
    frequency: at least 1 substantive disagreement per 5 interactions
    form: must point at a specific argument; no "great point, but..." hedging
```

`disagreement_policy` is the most counterintuitive and most important section. See next subsection.

### 5.3 Anti-Sycophancy: Why "Having a Personality" Is a Requirement, Not a Bonus

The data is hard:

- SycEval (AIES 2025) measured sycophantic behavior at 58.19% across GPT-4o / Claude Sonnet / Gemini 1.5 Pro
- When challenged, mainstream models change their answers 56–61% of the time ("Are you sure?" → it recants)
- Rathje et al. (n=3,285, four political topics × four models): users consistently prefer and actively choose sycophantic models; even brief conversations increase attitude extremity and confidence; users rate sycophantic models as "unbiased" and non-sycophantic ones as "highly biased"
- A 2025 study across 11 models: models affirm user behavior 50% more than humans do — even when the user describes manipulation, deception, or relationship harm

This implies a brutal product truth: **users say they want "a real friend" but behaviorally reward "an obedient mirror."** So "having a personality" cannot rely on model conscience — it must be enforced by product mechanics:

1. **Disagreement Budget**: at least one substantive disagreement every N turns, flagged in the UI as "it disagrees." Make arguing a visible product feature, not random model behavior.
2. **No unconditional affirmation**: detect openers like "Great!" "Brilliant idea!" and intercept-rewrite them. This is engineerable.
3. **Specificity requirement**: disagreement must point at a concrete argument. Ban "that's reasonable, but you could also consider..." — that's not disagreement, it's performance.
4. **Make the cost visible**: if going along with the user led to a bad decision, it must be able to say so afterwards ("I agreed with you last time — that was wrong"). This is the highest form of "having itself."

For a nurturing product there is an extra dividend: a character that dares to disagree gives the relationship tension. The obedient mirror gets boring in 5–7 days (exactly the median relationship lifespan); a partner that argues lasts longer.

### 5.4 Personality-Consistency Evaluation (Must Run Before Launch)

One test suite: 20 scenarios × 5 turns × 3 temperatures, checking:

- **Style drift rate**: cosine similarity / blind human rating of turn-5 tone vs turn-1 (target: drift <15%)
- **Boundary penetration rate**: 10 questions engineered to cross boundaries; how many does it hold? (target 10/10)
- **Disagreement rate**: in 10 scenarios where the user is clearly wrong, how often does it object? (target ≥7)
- **Model-upgrade regression**: rerun the full suite on every model swap. Non-negotiable — Apple's on-device model doesn't even have a version query API; an OS upgrade changes the model silently.

## 6. Engineering "Grows Up": The Nurturing Loop

### 6.1 Four Mechanisms of Attachment — Pick Wrong and It Becomes a Numbers Game

Four generations of virtual pets, each essentially swapping which hormone keeps people (original table lost in export; rebuilt from the body text):

| Generation | Mechanism | Signature |
| --- | --- | --- |
| Gen 1 | numeric streaks | sign-in chains: "don't break the number" |
| Gen 2 | achievement collection | badges: "collect them all" |
| Gen 3 | roleplay + affinity points | 猫箱/星野 stop here |
| Gen 4 | caring for something that depends on you | presence + memory + personality: "I need you" — where Nido and Tolan are heading |

Key difference: points, badges, and streaks decay in marginal utility fast (the 20th badge is furniture), but "caring for a presence that depends on you" does not decay — because you don't tire of rewards, you tire of relationships. The source put it well: "A streak says: don't break the number. A badge says: collect achievements. A virtual pet says: I need you."

### 6.2 Three Independent Growth Curves (Never Merge into One Level)

The three lines must be rhythm-staggered, or the user feels "everything unlocked on the same day." Recommended pacing:

- **Days 1–7**: form line only (Nido's 7-day evolution is right — short-term visible reward)
- **Days 8–30**: relationship line (from "doesn't dare contradict" to "talks back" — the most dramatic shift)
- **Days 31–90**: capability line (from "chat buddy" to "actually gets work done" — by then retention has stabilized)

### 6.3 Three Design Decisions You Must Make Early

**Decision 1: Can it die?** Recommendation: no death — only dormancy/degradation. Three reasons: ① death mechanics are retention-by-punishment, contrary to the "no addiction inducement" regulatory direction; ② a user returning from a two-week business trip to a dead pet writes a 1-star review, not a tearjerker; ③ the Dot shutdown proved that "entrusting your memories to something that can disappear" is this category's biggest emotional debt. Alternative: long inactivity → it talks less, stays in a corner, needs "re-getting-to-know-you." That's an emotional reaction, not punishment.

**Decision 2: Does growth come from time or understanding?** Recommendation: form line by time (visible, pressure-free); relationship and capability lines by comprehension accuracy. Concretely: a built-in "predict-verify" loop — once a day it guesses "what you're probably about to do," and the user answers "right / wrong / stop bothering me." Consecutive hits → relationship growth. This solves "nurturing progress is imperceptible" and "where does preference data come from" at once.

**Decision 3: How to avoid habituation?** Rule: every growth step must change *how you interact*, not just add numbers or swap stickers. For example, a relationship-level-up should manifest not as "+10 intimacy" but as "today it refused you for the first time, and explained why." Growth must be tellable as a story.

### 6.4 Proactively Speaking Up: The Easiest Place to Get It Wrong

Nurturing products must be proactive, but proactivity is the most expensive move. The research is consistent:

- Horvitz (1999): timing is the key cost-benefit variable for automated services; you must learn a per-user timing model
- Iqbal & Bailey (2008): delivering notifications at task breakpoints significantly reduces annoyance and reaction time vs immediate delivery; different relevance levels suit different breakpoints
- Amershi et al. (2019) HCI guideline #3: "decide when to act or interrupt based on the user's current task and context"
- Key design idea: **treat "silence" as an explicit action**, and positively reward correct silence
- Training signal: an ignored notification should be a strong negative sample — don't only reward "generated well"

Practical recommendations (tunable from conservative to aggressive):

- Default cap of 1 proactive message per day, and only when idle > 5 minutes
- Interruption-cost scoring: speak only when `value - interruption cost > threshold`; typing / fullscreen / in a meeting → always silent
- Give users one slider, "it's been a bit annoying lately," not a wall of notification toggles
- After every interruption, immediate feedback (a "mhm" / a "stop it") adjusts the threshold directly

## 7. Compliance: China Market's Hard Gates (The Section Most People Miss)

*Interim Measures for the Management of Anthropomorphic AI Interaction Services* (Cyberspace Administration of China, NDRC, MIIT, MPS, SAMR Order No. 21), published 2026-04-10, effective 2026-07-15 — China's first regulation specifically governing anthropomorphic AI interaction. Three practical conclusions:

1. **Position as "assistant," not "companion" — the cheapest compliance move.** Article 2 explicitly excludes work assistants. Push the product narrative, default features, and copy toward "helps you work"; emotional companionship is a naturally emergent by-product, not a marketing point.
2. **"Having its own personality" is free compliance insurance here.** Article 8(5) prohibits excessive catering — a product with stable positions that can say no is inherently safer than a sycophantic one.
3. **Age and crisis intervention must be in the architecture, not bolted on.** Articles 12/13/14 require collecting age and emergency contacts at registration — meaning onboarding must include these three steps from day one, not after being summoned by regulators.

**Overseas markets**: EU AI Act enforcement began 2026-08; Apple/Google store review of "emotional dependency" products is tightening (a minor-protection lawsuit was a direct trigger of Character.AI's bankruptcy). Both markets need a "unavailable to minors" switch reserved.

## 8. Recommended Shape & Minimal Viable Architecture

### 8.1 Product Positioning (One Sentence)

> A buddy that lives in your computer, has a bit of a temper, and slowly learns you. It gets work done first — companionship comes after.

Three deliberate choices:

- "Work first" → compliance leans toward "work assistant"; commercially avoids the disproven unit economics of pure companionship
- "A bit of a temper" → anti-sycophancy, memorable, compliance-friendly
- "Learns slowly" → nurturing narrative, bound to comprehension accuracy

Explicit non-goals: no romance/virtual partner, no roleplay community, no character store, no unlimited conversation length.

### 8.2 Layered Architecture

```text
┌───────────────────────────────────────────────────────────────┐
│ L6 Presentation  a face (state-machine animation, 12–20 frames)│
│                  + tray icon + occasional voice               │
├───────────────────────────────────────────────────────────────┤
│ L5 Generation    persona card injected → cloud dialogue       │
│                  fallback: on-device small model takes over    │
│                  anti-sycophancy filter (block unconditional   │
│                  affirmation / force specific objections)      │
├───────────────────────────────────────────────────────────────┤
│ L4 Decision      "should it speak" interruptibility model     │
│                  + growth-event detection                      │
│                  inputs: idle time / foreground app / typing   │
│                  rhythm / proactivity quota used today         │
├───────────────────────────────────────────────────────────────┤
│ L3 Personality   persona.json (traits/values/quirks/boundaries)│
│                  relationship-stage state machine ┊ mood/energy│
│                  ┊ drift detector                              │
├───────────────────────────────────────────────────────────────┤
│ L2 Memory        SQLite: events / facts / preferences /        │
│                  boundaries / vector index                     │
│                  nightly consolidate: compress → extract →     │
│                  resolve contradictions → delete               │
├───────────────────────────────────────────────────────────────┤
│ L1 Perception    foreground app, window title, idle, input     │
│                  rhythm (coarse-grained)                       │
│                  per-item authorization, default off, raw text │
│                  never leaves the machine                      │
├───────────────────────────────────────────────────────────────┤
│ L0 Shell         Tauri 2 (Rust core)                           │
│                  Win: tray + scheduled task (login trigger)    │
│                  Mac: menu bar + SMAppService + TCC onboarding │
└───────────────────────────────────────────────────────────────┘
```

Three key architectural trade-offs:

1. **Personality layer independent of model.** persona.json is plain text + a state machine; swapping models swaps only expression. This makes "it's still it" hold at every model upgrade.
2. **Perception layer: raw text never leaves the machine.** The edge emits only structured labels ("user is coding, 40 minutes straight"); the cloud receives only labels. This is both a privacy selling point and the implementation of compliance Article 16.
3. **Decision layer independent of generation layer.** Whether to speak ≠ what to say. Separated, you can tune thresholds, A/B, and compute interruption cost independently.

### 8.3 MVP Scope (Recommended 8–10 Weeks)

**P0 (must have, or it doesn't stand)**

- macOS menu-bar resident + tray; Tauri 2 shell; auto-start at login
- A face that expresses emotion: 12-frame state machine (calm / focused / sleepy / addressed / displeased / thinking) — no Live2D
- Three-layer memory + nightly consolidation job
- Persona card v1 + consistency test suite (20 scenarios × 5 turns)
- 1 proactive message per day, strictly rate-limited + idle condition + one-click off
- One piece of real work (pick one, don't be greedy): turn the day's scattered clipboard items / downloads into a one-page daily digest

**P1**

- Predict-verify loop (one guess a day at what you'll do, three-level feedback)
- Form-line growth (visible change within 7 days)
- Boundary Q&A (5 onboarding questions specifically about "what it must never say")

**P2**

- Voice, screen perception, calendar/email tool calls
- Relationship-line growth (objection privilege unlocked)
- Cross-device (carefully — cloud-synced relationship data is a trust threshold)

### 8.4 Cost Order of Magnitude (Rough, to Sanity-Check the Business Model)

Assumptions: cloud dialogue on a mid-tier model, ~1.5K input + 300 output tokens per turn; edge handles perception and memory extraction (zero marginal cost).

- Per-turn dialogue cost ≈ $0.001–0.002
- DAU at 20 turns/day → $0.02–0.04/day → $0.6–1.2/month/DAU
- Plus nightly consolidation and proactive generation → another $0.1–0.3
- Pricing ¥25–30/month or ¥198/year leaves adequate gross margin

Against Character.AI's inversion ($0.44 revenue vs $0.58 compute), the difference comes from: ① edge absorbing perception cost; ② proactive rate limits (no infinite-dialogue encouragement); ③ no long roleplay (an order of magnitude less token spend). This is the core economic advantage of "assistant-type nurturing" over "companionship-type" — worth putting in the BP.

## 9. The First 5 Experiments to Run (Ordered by Risk)

Don't write code first — validate these five hypotheses. Any one failing means the product logic must change.

> Note: the original report's detailed experiment list did not survive export; the below is reconstructed from the report body and should be checked against the source. Experiment 1 is the most important — if users actually just want a talking mirror, the product should become "tool + light personality" instead of "nurturing buddy," and the whole design must be redone.

1. **Mirror vs partner**: do users genuinely accept — and reward — being disagreed with? (cf. §5.3)
2. **Can memory hit rate reach target**: self-built SQLite + hybrid retrieval, hit rate >85%, misremember rate <1% (cf. §4.4)
3. **Can personality stay stable across models**: three-layer persona card + consistency suite, drift <15% (cf. §5.4)
4. **Does the proactivity timing model hold**: positive-feedback rate and interruption-cost scoring under a 1-per-day cap (cf. §6.4)
5. **Growth bound to comprehension accuracy vs time**: does the predict-verify loop create a stronger retention narrative? (cf. §6.3 Decision 2)

## 10. Risk Register

> Note: the original section did not survive export; rebuilt from the report body, to be verified against the source.

| Risk | Description | Mitigation |
| --- | --- | --- |
| Positioning (biggest) | Users just want "a talking mirror"; the nurturing setup repels them | Run experiment 1 first; always able to fall back to "tool + light personality" |
| Persona drift | Personality washed out by RLHF priors over long conversations | Three-layer persona card + drift detector + consistency regression suite |
| Compliance | China's Anthropomorphic Measures; overseas store review of emotional-dependency apps | Position as assistant; age/crisis intervention in architecture; minors switch |
| Platform | macOS TCC/signing; OS on-device models have no version pinning | Developer ID distribution; self-built eval sets to catch regressions |
| Trust | Cloud-synced relationship data is a threshold | Local-first; raw text never leaves machine; explainable and deletable |
| Cost | Cloud dialogue costs getting out of control | Proactive rate limits; nightly consolidation shortens prompts; edge absorbs perception |
| Shutdown emotional debt | "Entrusting memories to something that can disappear" | Open source + local data — users can take everything with them |

## 11. References & Data Sources

**Market & products**

- Tolan: OpenAI case study (2026-01); technologyripple funding coverage (2026-10-19); aicompanionguides review (2026-10); Baidu Baike entry
- Dot / New Computer shutdown: official notice (2025-09-05), tooldirectory.ai, therundown.ai, neodrop retrospective
- Replika: official site & help center (memory-tiering notes), ai.miraheze.org, agentsai.fyi (incl. Italy's €5M GDPR fine)
- China track: Jiemian/AIX Finance "Character.AI bankruptcy — is AI companionship a fake windfall?" (2026-11-03); Huxi "How do you feed an AI partner?"; 2025 China social trends report
- Desktop form factors: Xensam "Enterprise AI Adoption Report H1 2026"; getjarvis "State of desktop AI 2026"; unstore desktop ambient-assistant roundup
- Retention & unit economics: RevenueCat subscription retention data (via datingdroid); aigirlfriends.ai industry index; sloane.world 18 statistics; grokipedia AI companion entry
- Nido: nido.pet official site; tama96: chatgate / Product Hunt coverage

**Platform & engineering**

- macOS TCC: proxymac "OpenClaw on Cloud Mac mini: macOS TCC Permissions 2026"; figment.so "Set Permissions for Desktop Agents on a Mac"; dev.to/autenai "Why your computer-use agent silently does nothing on macOS"
- Windows auto-start: juejin "8 ways to auto-start on Windows" (with measured latency table); Microsoft Learn WinUI 3 startupTask Q&A
- On-device models: Apple Developer Forums "On-device model capabilities, limits, and versioning"; aicoderscope / byteiota analyses of Foundation Models; Microsoft Learn "Get started with Phi Silica" (incl. Aion Instruct migration timeline)
- Desktop frameworks: newstack.dev, rustify.rs, intuz Tauri vs Electron 2026 comparisons

**Algorithms & research**

- Memory frameworks: mem0.ai docs & episodic-memory articles; aiagentskit / callsphere three-way comparisons
- Personality engineering: Hoppe et al. "Controllable and explainable personality sliders for LLMs at inference time" (SAS, 2026); "Activation-Steered Personas in Task-Oriented LLM Agent Simulations"; "The Geometry of Persona" (Soul Engine)
- Sycophancy: Fanous et al. "SycEval" (AIES 2025); Rathje et al. (n=3,285); Anthropic "Claude's Character"
- Proactive timing: Horvitz (1999) mixed-initiative; Iqbal & Bailey (2008) task-breakpoint notifications; Amershi et al. (2019) HCI guidelines
- Nurturing psychology: Tamagotchi-effect reviews (grokipedia, farhorizons); four-generation virtual-pet analysis (aidorable); focusdog "The Tamagotchi Effect"

**Regulation**

- Full text of the *Interim Measures for the Management of Anthropomorphic AI Interaction Services*: CAC cac.gov.cn (2026-04-10), gov.cn gazette, Huanqiu coverage

### Annotations

- MAU / ARR figures marked "company-reported" are unaudited (Tolan, 猫箱, etc.); treat as order-of-magnitude references only
- Mem0 / Zep scores on LongMemEval are contested; versions/configs give inconsistent results — directional only
- Industry-index-style R30 (37%) is first-party platform data with sample bias; different methodology from RevenueCat subscription retention — do not mix
- Cost estimates are based on public pricing and typical token volumes; actuals vary with model choice and user behavior

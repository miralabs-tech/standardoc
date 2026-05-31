# Notes

📖 English · [Français](../../fr/storytelling/remarques.md)

[← Philosophy](philosophy.md) · [Short-term vision](vision-short-term.md) · [Mid-term vision](vision-mid-term.md) · [Long-term vision](vision-long-term.md) · [Test feedback](test-feedback.md)

> This document gathers the **cross-cutting syntheses** from the
> development sessions: structural decisions that came back several
> times, dogfood learnings, agent usage patterns, avoided
> anti-patterns, and the posture that follows from the project's
> material reality.
>
> It's not a chronological inventory of the memos — it's what stands
> out in relief from them.

---

## Structural decisions

Choices that came back across several memos and that structure the
project end to end:

- **Canonical IR + direct AST as the moat.** The project explicitly
  rejects the per-IDE LSP and the surface-level tree-sitter used
  elsewhere: "deep `syn` / `swc` AST (signatures, types, generics,
  traits) vs. surface tree-sitter (regex-like, functions + classes +
  calls only)". It's the central differential against the existing
  tools — not an opinion decision, an invariant decision.

- **License-as-moat (FSL-1.1-MIT → MIT 2028).** A central lock
  re-locked several times. Plain MIT was ruled out (no short-term
  protection), AGPL too (insufficient against closed-source non-SaaS
  competitors who would fork without publishing). FSL with **automatic
  conversion to plain MIT 2 years later, per release**, is the only
  mechanism that combines initial protection and an irreversible
  commitment to openness.

- **The graph is not agent memory.** The code graph (`.standardoc/`) was
  always kept separate from agent session memory; in beta.3 that memory —
  and the RAG prose layer that lived beside it — moved out of the core
  entirely. The core derives from code, nothing else.

- **Primitives first, conventions after.** A dogfood pattern observed
  over several cycles: we lay down the stable primitive (the
  `enrichments` table, the `BridgeKind` tag, typed edges) **before**
  filling all its consumers. Rationale: a well-laid primitive absorbs
  10 iterations of conventions without breaking; a convention frozen
  too early calcifies the tool (cf. [mid-term
  vision](vision-mid-term.md)).

---

## Dogfood learnings

What worked unexpectedly, what failed, the scope pivots.

- **The beta.2 → beta.3 pivot on doc rendering.** Original plan: beta.2
  = doc rendering layer + self-managed CLI. Reality: dogfood surfaced
  more urgent needs (MCP surface too thin, sessions evaporating between
  chats, daemon fragile under orchestration). Doc rendering slipped to
  beta.3 — **not out of procrastination, out of a different priority
  revealed by real usage**.

- **Infra surprise: the HTTP/SSE transport.** In beta.1, MCP stdio
  created a child standardoc per chat window: "5 chats = 5 children, a
  RAM hog". The move to HTTP/SSE in beta.2 collapsed that: "2 processes
  per VSCode window, independent of the number of chats". An unforeseen
  side effect: the parent-death-watch via stdin pipe eliminated all
  orphans (kill VSCode, BSOD, OOM — the child kills itself).

- **Owned failures and cuts.** Several leads were killed without regret
  over the project — the v0 `{{ @doc.X }}` templating DSL, the
  `materialize` command, the separate `standardoc-server` binary, the
  `.standardoc.json` config file, the initial publish to crates.io
  (details in [test feedback → what we dropped](test-feedback.md)).
  Each cut freed up scope for what became beta.2.

---

## Agent usage patterns beyond calibration

Complements to [test feedback](test-feedback.md) — patterns
specifically observed on the agent's consumption of the graph:

- **Strict MCP-first with the 3-phase protocol** (`find` → `context` →
  `body`). AST graph navigation **before** Grep, always. Grep reserved
  for "true string-literal targets with no graph anchor" (comments,
  out-of-code configs, build files).

- **Probe discipline.** "One single targeted probe, accept `[]` as a
  'doesn't exist' signal". An explicitly refused anti-pattern: 4
  sequential `*foo*` / `*bar_foo*` variants when the first already
  returned a `did_you_mean` suggesting the right lead.

- **POUR / CONTRE / VOTE on any design with more than one viable
  option.** When the operator can decide, they decide; when only the AI
  sees the technical nuance, the AI decides and documents. This
  convention avoids bikesheds on choices with no real impact.

---

## Avoided anti-patterns on the work side

Different from the *"what we do NOT do"* on the product side (already in
the visions) — here, the **daily work anti-patterns** we refuse:

- **Dummy items to host annotations.** When a language lacks a
  primitive (for example Lua with no native type system), the
  temptation is to introduce stub functions / values just to attach
  annotations to them. A flat refusal: "pollute the source with dead
  code that makes no sense". If the annotation can't be attached to a
  living symbol, it lives elsewhere (the enrichments table, a sidecar).

- **Time estimates in days / weeks.** An owned observation: "my
  estimates are systematically overstated by an order of magnitude".
  Consequence: we don't give a firm calendar on the work items; the
  labels are structural (beta.2 / beta.3 / 1.0) or conditional
  ("dogfood-driven, may slip"), never dated to the week.

- **BUSINESS-MODEL out of marketing before 1.0.** Standardoc's public
  differentiator stays DX / perf / direct AST. The internal discussions
  on future pricing, sponsoring, the hypothetical post-1.0 SaaS pivot —
  all of that stays **out of public discourse** until 1.0. No teasing,
  no ambiguity.

- **Re-asking a question already decided.** A locked decision isn't
  re-litigated on every new session — unless a new fact invalidates its
  basis. It stays explicitly superseded, not replaced silently.

---

## Context

Standardoc is open-source, local, with no user tracking, and maintained
solo under [miralabs.tech](https://miralabs.tech) (the entity behind the
`miralabs-tech` org). It's part of **StandarX**, a set of OSS tools that
standardize structural problems of the dev ecosystem; Standardoc is the
first out publicly.

It ships now because two things lined up: AI agents became reliable enough
to amplify a solo maintainer (MCP, large context windows, hooks), and the
underlying problem — every tool re-parsing the code, drift between indexes,
cross-language cognitive debt — got bad enough to be worth solving properly.

### Contributing & support (pre-1.0)

- **No third-party PRs until the 1.0 freeze** — the API surface has to
  stabilize cleanly first. **Issues, feedback, and ideas are welcome** via
  GitHub Issues / Discussions. Post-1.0 opens up (the UST + Lua plug-in
  layer is built for exactly that — community providers without touching
  the frozen core).
- Support via [StandarX on OpenCollective](https://opencollective.com/standarx)
  goes straight into shipping speed.

---

## Going further

- **[← Philosophy](philosophy.md)** — the 5 system-thinking principles
  and the construction ethics
- **[Short-term vision](vision-short-term.md)** — beta.2 and the
  stabilization phase
- **[Mid-term vision](vision-mid-term.md)** — beta.3 and 1.0
- **[Long-term vision](vision-long-term.md)** — UST + Lua plug-in layer
  post-1.0
- **[Test feedback](test-feedback.md)** — agent calibration, what we
  dropped, dogfood observations
- **[TODO-LIST](../TODO-LIST.md)** — exhaustive checkboxes per milestone

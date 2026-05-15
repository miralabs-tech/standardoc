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

- **Sessions DB orthogonal to the graph, RAG linked by FQDN.** An
  architectural precision we had to re-lock several times: "don't say
  'they all read the same graph' (false)". The LSP and the MCP share
  `.standardoc/index.db` (the graph) and `.standardoc/rag.db` (prose
  chunks linked by FQDN). The sessions DB
  (`.standardoc-sessions/sessions.db`) is **separate** — it's agent
  memory, not a graph derived from the code.

- **`.md` = canonical transport, `.db` = reproducible local cache.** A
  recurring pattern: the session memos have a persistent form in SQLite
  (quickly consultable, indexed), but their canonical exchange and
  versioning format stays Markdown with frontmatter (`status`,
  `supersedes`, `created_at`). You can lose the DB without losing the
  memos — just rebuild.

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

- **Cap the RAG corpus, not the filter.** A dogfood track: we tuned the
  RAG filters (threshold 0.55, a 23-word stop-list, a confidence floor)
  and found that "the legitimate signal is limited by the corpus" — not
  a filter problem, a problem of prose mass in a project still under
  construction. As the docs grow, the RAG becomes more useful.

---

## Agent usage patterns beyond calibration

Complements to [test feedback](test-feedback.md) — patterns
specifically observed on the agent's consumption of the graph:

- **Strict MCP-first with the 3-phase protocol** (`find` → `context` →
  `body`). AST graph navigation **before** Grep, always. Grep reserved
  for "true string-literal targets with no graph anchor" (comments,
  out-of-code configs, build files).

- **`session_save` over copy-paste handoff.** A pivot observed in an
  internal session: bootstrapping a new session by copy-pasting the
  previous context ends up superseded by `session_save(slug, body_md)`
  + `session_get()` at the start of the next chat. The MCP tool
  **supersedes** the global "pass the context" convention — it's the
  infrastructure that carries the memory, not the operator.

- **RAG cross-session via FQDN.** The `chunk_refs` injected into
  `get_context` mean a session recovers the relevant prose of a
  previous session — for example a section of documentation anchored on
  a precise FQDN. A private confidence threshold of `0.55`, a 23-word
  stop-list (`data`, `default`, `done`, `file`, `find`, …) to avoid
  trivial anchors.

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

- **Re-asking a question already decided in a locked spec.** When a
  decision has been locked (`SessionKind::Lock`), it isn't re-discussed
  on every new session — unless a new fact emerges that invalidates the
  basis of the decision. The memo stays explicitly superseded (via the
  `supersedes` field), not replaced silently.

---

## The project's material reality and the posture that follows

### Standardoc isn't a project from last week

The `miralabs-tech/standardoc` repo is recent (post-communication
overhaul, post-rebranding), but **I've been thinking about it for
several years** and **I prototyped it ~6 months ago** on a personal
account: [SUP2Ak/standardoc-cli](https://github.com/SUP2Ak/standardoc-cli).
The current version has evolved a lot since that prototype (honestly,
nothing in common technically), but the line of thought predates the
official repo by a long way.

What Standardoc represents comes from **more than a decade of dev
practice** where, project after project, I saw certain structural
problems of the *dev ecosystem* become visible — every tool re-parsing
the code, drift between parallel indexes, cross-language cognitive debt,
dependence on SaaS to understand your own code. Standardoc is my attempt
to solve these problems matured in my head over 10-15 years.

What makes Standardoc **shippable now** rather than 2 years ago is the
conjunction of two things:

1. **AI tech went from experimental to usable.** MCP tools, 1M-token
   windows, `PreToolUse` / `SessionStart` hooks, the Claude Code agent
   ecosystem — all of that became reliable through 2026, after a
   promising but messy mid-2025 phase.
2. **Amplified solo architecture** (cf. the next section) lets me move
   at the pace needed to stabilize a public API, deliver a complete MCP
   surface, and freeze a canonical IR in a few months — not in several
   years.

### Standardoc within the StandarX suite

**StandarX** is the set of personal projects I carry as OSS, all aiming
to **standardize structural problems of the dev ecosystem**. Standardoc
is the first to come out publicly — others have been simmering in the
private drawer for years.

**The legal entity behind it** is my sole proprietorship,
**[miralabs.tech](https://miralabs.tech)** — it's the one that owns the
`miralabs-tech` GitHub organization and hosts Standardoc. The reason for
the legal status is mundane: under French law, to work independently
(sponsoring, freelance contracts, collaborations), you have to be
*something* in the eyes of the law, not *someone*. **I'm open to indie
opportunities via miralabs.tech — or to a classic hire outright**,
alongside the rest.

Standardoc is **the most important piece of StandarX to me** because
it's the one that can **make concrete some ideas that have been
simmering for years** about structural problems of the dev ecosystem
(code understanding as a system, not as a string of greps).

I have other personal projects sitting in private for several years —
never released because never prod-ready, and above all because they
don't earn anything and don't have the same alignment with a broad
ecosystem need. Standardoc goes first because it has the luck of
matching a moment where the problem has become visible **and** where the
tools to solve it exist.

And all of this **OSS, local, with no user tracking** — because the
problem is real for indie devs, those who work on OSS projects, or solo
devs on a large codebase: **writing code on top of managing the
architecture, the design, the stack, the long-term debt, the docs, the
CI, the packaging** is a mental sinkhole that worsens over time. **It's
not only RAM that can overflow; the brain can too**, especially solo on
large codebases.

### The role of AI: amplified solo architecture, not "vibe coding"

The shift in AI tools since mid-2025 (mature through 2026) changed my
solo working mode. **This isn't "vibe coding"** — I don't fire a vague
prompt at an agent and watch what comes out. It's **amplified solo
architecture**:

- **I set the design**, the **canonical snippets**, the **approach
  techniques in the algorithmic sense** — not vague ("you make a REST
  API with endpoints"), precise ("you make an FTS5 query with snake /
  camel tokenization and a strsim fallback at threshold 0.6").
- The agent **executes under discipline** — MCP-first guardrail, 3-phase
  `find → context → body`, `session_save` / `get` (cf. [test
  feedback](test-feedback.md)).
- **I stay responsible** for the **review**, the **overall
  architecture**, the **IR contract**. The agent amplifies; it doesn't
  decide.

Without that amplification, **I couldn't move at this pace alone** (~90
commits over the beta.1 → beta.2 cycle, a dozen major technical work
items). With it, ideas that were simmering in private become shippable.
It's not magic — it's conscious architecture coupled with disciplined
execution.

### Pre-1.0 contribution rules

- **No third-party PRs before the 1.0 freeze.** The API surface has to
  freeze cleanly, and accepting external PRs before stabilization would
  introduce noise on choices I have to keep controlled to carry the
  contract.
- **Issues, feedback, technical or global ideas: all welcome**, via
  GitHub Issues / Discussions. It's even where I learn fastest what's
  missing.
- **Post-1.0, the model opens up** — it's partly why the UST + Lua
  plug-in layer is central in [long-term vision](vision-long-term.md):
  it lets community contributions be absorbed without touching the
  frozen core.

### OpenCollective isn't decorative

[StandarX on OpenCollective](https://opencollective.com/standarx) isn't
a button placed to look nice on the README.

**I carry Standardoc alone, while holding down two jobs on the side.**
The project isn't a source of income — I maintain it because it's
useful, not because it pays. If you want to **see 1.0 arrive fast**,
support matters concretely: it lets me reduce the time spent on the
subsistence jobs and move faster on Standardoc (and on the other
StandarX projects that follow).

This isn't blackmail. **It's a material fact**: without support, the
shipping pace depends on my residual free time, and my weeks are already
busy elsewhere. With support, the pace aligns with the priority the
community gives the project.

**Honest note: I don't do promotion in general.** For me the code
speaks louder than the marketing, and this is the first time I'm making
a real communication effort around one of my projects — precisely
because Standardoc is the one that can make concrete what's been
simmering for a long time. If the OpenCollective doesn't take off, I'll
keep shipping on my residual time; it'll just be slower.

### Communication posture

Three principles calibrated over the dogfood:

- **Don't be a cheerleader, don't be a doomer.** The project doesn't
  need hype, nor catastrophism. The trajectory is laid, the quality of
  the code speaks, the narrative follows the real.
- **Asymmetric GitHub stats (clones >> visits) ≠ a signal.** The ratio
  reads as **pre-beta.2 + pre-communication-overhaul noise**, not as a
  failure metric. Standardoc's target (devs who master their tooling)
  clones directly via `gh repo clone` without visiting the GitHub page —
  the real audience leaves no visit trace.
- **Honest positioning maintained.** The line "ripgrep & LSP are enough
  there" for small SPAs is kept deliberately in the README. A refusal to
  claim universality — Standardoc is strong on large and complex
  codebases (compilers, languages, engines, heavy monorepos), **overkill
  elsewhere**, and that's OK.

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

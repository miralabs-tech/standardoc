# Short-term vision — beta.2 → 1.0

📖 English · [Français](../../fr/storytelling/vision-court-terme.md)

[← Philosophy](philosophy.md) · [Mid-term vision →](vision-mid-term.md) · [Long-term vision →](vision-long-term.md)

> This document is the **narrative** behind the short-term milestones.
> The exhaustive feature list per milestone lives in
> [`TODO-LIST.md`](../TODO-LIST.md) — that's the one that moves, not
> this doc.

---

## Where we are

Standardoc is coming out of **v1.0.0-beta.1**. That foundation included
direct AST parsing for Rust + TS, a cross-language canonical IR, SQLite
+ FTS5 + file watcher, LSP/MCP daemons, the VSCode extension, and the
distribution infra (cross-platform releases, version.json manifest,
FSL-1.1-MIT).

beta.1 is **stable**. What ships on `main` (Rust + TS, 2 MCP tools, LSP,
VSCode ext) is usable in production and will stay that way — we don't
rip anything out retroactively.

beta.2 isn't a release of "new features we want to sell". **It's a
maturity release** — beta.1's foundation put under the pressure of real
usage across 3 dogfood targets in parallel, and refined until it holds
under loads we hadn't imagined at the start.

---

## The dogfood targets during the beta.1 → beta.2 phase

- **Standardoc itself** — indexed in its own CI; if a PR breaks the IR or
  the MCP surface, CI catches it before merge. If the tool isn't usable on
  itself, it's usable on nothing.
- **Two other projects** — a polyglot build engine and a homegrown
  language (Rust + C). They stressed the `get_body` knobs (`strip_attrs`,
  `signature_only`) on heavy handlers, and validated the "multi-language
  monorepo with a part outside the graph" pattern: the agent uses the
  indexed part and falls back to documented `Read` on the rest, without
  confusion.

---

## The scope pivot between the original plan and reality

The original plan for beta.2 was: **doc rendering layer + CLI
self-management**. On paper, it was coherent — replace the DSL
templating killed in beta.1, and make `standardoc` self-sufficient
without VSCode.

In practice, none of what was planned got built in this phase. **Not
out of procrastination — out of higher-priority needs revealed by real
dogfood**: an MCP surface too thin for 90% of agent flows, sessions
evaporating between chats, agents shortcutting to grep the moment they
can, stdio MCP limited to 1 client at a time, adjacent prose
inaccessible to the graph, languages beyond Rust+TS missing, the daemon
not resilient enough under real orchestration.

So the scope was re-aligned on what actually emerged: **hardening + MCP
surface refinement**. Doc rendering and the self-managed CLI moved to
beta.3.

**Not out of abandonment — out of a different priority.** We don't ship
a rendering layer if the layer underneath is still refinable.

---

## beta.2 — what it really represents

Three fundamental angles that mark the phase:

### 1. The MCP surface is no longer a placeholder, it's a toolkit

We go from a day-1 surface (2 tools) to a surface usable in production
under real agent flows. The criterion isn't the number of tools — it's
that **no hole observed in dogfood is left unanswered on the API side**.
When the agent has to do a cross-module audit, when it needs a compact
read of a handler, when it types an approximate FQDN, when it wants to
verify that its knowledge of a symbol is still fresh — there's a tool
for it. And when the agent burns context by skipping the recommended
pacing (`get_context(depth=2)` without a prior `depth=1`), the server
returns a corrective `routing_hint` instead of letting it continue
silently.

### 2. The multi-frontend / multi-backend architecture left the whiteboard

beta.1 had LSP + MCP stdio. beta.2 validates in practice: MCP HTTP/SSE
multi-client, extended language providers (Lua, Vue, Svelte). The
challenge isn't each piece individually — it's that **the whole stays
coherent**, without any surface corrupting another's state.

It's validated now. No longer in theory.

### 3. Discipline became a feature, not a convention

The **MCP-first guardrail** turns a good intention ("the agent should
use MCP first") into an observable rule of the system, via the Claude
Code hooks (PreToolUse + SessionStart + mark sentinel). No more "the
agent should" — the agent **must** or it's blocked. And the block is
observable (deny with a structured message), not silent.

It's the first brick of a broader approach: encode good-behavior
patterns into the system, not into good intentions. The other bricks
(routing_hint, daemon-side enforcement) follow the same logic.

→ [Exhaustive beta.2 feature details in TODO-LIST](../TODO-LIST.md)

---

## From beta.2 to 1.0 — the stabilization phase

beta.2 → 1.0 isn't an explosion of features. It's the passage from a
tool **we refine** to a tool **we contractualize**.

At 1.0, the public API (MCP tool signatures, LSP custom methods, IR
types exported by `standardoc-ir`, SQLite schema) is **frozen**. That
means:

- Every later breaking change goes through a `protocol_version` bump
  and a coexistence period
- Tools don't disappear silently
- The SQLite schema only migrates forward (never downgrade)
- Scale benchmarks are **published** (cold start, watcher delta, MCP
  query latency p99 on 1M+ LOC monorepos) — no "it scales, trust us"

It's the contract that lets third parties build on it with confidence.
As long as we're not at 1.0, we keep the right to move a tool's
semantics; at 1.0 that right is extinguished without explicit agreement
with users.

→ [Detailed 1.0 roadmap in TODO-LIST](../TODO-LIST.md)

---

## The invariants we protect in this phase

Whatever happens between now and 1.0, these invariants don't move:

- **The IR stays stable.** The types in `standardoc-ir` (`RawSymbol`,
  `RawEdge`, `EdgeKind`, `ResolvedOrUnresolved`, …) are the project's
  cross-language and cross-decade grammar. We add to them if needed, we
  don't remove from them, and we don't change the semantics of an
  existing type without a `protocol_version` bump.
- **The graph stays local.** No cloud sync, no auth, no phone-home
  telemetry. Everything lives in `.standardoc/` (gitignored,
  reproducible).
- **The license timer stays armed.** Every release keeps the automatic
  FSL-1.1-MIT → plain MIT conversion 2 years after its date. The first
  release (`v1.0.0-beta.1`) converts on April 26, 2028. No retroactive
  change of terms is possible — that's the commitment we make.
- **The SQLite format stays open.** Versioned schema, dump-able with a
  standard sqlite3, readable without any proprietary tool. If we
  disappear tomorrow, your index keeps working.

---

## Going further

- [Mid-term vision →](vision-mid-term.md) — beta.3 (doc rendering layer,
  self-managed CLI) and 1.x (post-stabilization)
- [Long-term vision →](vision-long-term.md) — UST + Lua plugin layer,
  ecosystem, platform
- [Notes →](notes.md) — dogfood observations, locked decisions
- [Test feedback →](test-feedback.md) — what we tested, what we dropped,
  the measurements
- [TODO-LIST →](../TODO-LIST.md) — exhaustive checkboxes per milestone

# Philosophy

📖 English · [Français](../../fr/storytelling/philosophy.md)

[← Back to README](../../../README.md) · [Short-term vision](vision-short-term.md) · [Mid-term vision](vision-mid-term.md) · [Long-term vision](vision-long-term.md) · [Notes](notes.md) · [Test feedback](test-feedback.md)

---

## The problem we're trying to solve

**Code understanding is a system.** Not a string of searches. Not an
isolated AI agent that re-greps your codebase on every task. Not an LSP
that can answer `goto-definition` but forgets your intent two seconds
later. Not a Sourcegraph that indexes your repo in a distant cloud to
hand it back to you through a paid per-user API.

It's a **system** — meaning a set of components that talk to each other,
share a common representation, evolve together, and stay coherent over
time. When a code understanding system is built correctly, **any tool in
the ecosystem** (IDE, AI agent, doc generator, navigation dashboard,
review plugin) can consume the same source of truth without re-parsing
it on its own side, without drift, without a parallel cognitive debt.

Today, that system doesn't exist. Every tool builds its own:

- Your LSP parses your code → a per-IDE graph, dead between two openings
- Your AI agent greps your code → per-task archaeology, 30k tokens, zero
  carry-over between sessions
- Your doc generator parses your code → yet another AST, yet another
  invalidation, rotten again by the next update
- Your review tool parses your code → ad nauseam

**N times the same work, N times the same bugs, N times the same desync
between the truth (the source code) and the derived representations (the
scattered indexes).**

Standardoc is the attempt to collapse all that into **one shared
indexing pass**, with a canonical IR at the center, and as many
consumption surfaces as your workflow demands.

---

## The diagnosis — why current approaches don't hold

### LSPs are per-IDE and per-language

Tower-LSP, vscode-languageclient, tower-lsp-server, etc. — each IDE has
its own LSP integration, and each language has its own LSP server
(`rust-analyzer`, `tsserver`, `vue-language-server`). That's fine — for
an IDE. For an AI agent that wants to understand a **multi-language
codebase** (TS consuming a Rust lib via WASM, Vue calling TS components,
Lua requiring a Rust module), LSPs give you only a partial, fragmented
view, with an API designed for a human clicking in an IDE, not for an
agent issuing queries.

### Regex / text tools rot

`grep`, `ripgrep`, ctags, standalone tree-sitter — it's fast, it's
portable, but it's **fragile the moment the code mutates**. A variable
rename, a namespace refactor, a framework migration, and all the regex
heuristics and naming conventions shatter. Maintaining regex tooling on
a large codebase is permanent maintenance of a cache that desyncs all
the time.

### AI agents in `grep + read` mode don't carry over

The agent opens a session. It greps, it reads, it accumulates 30k tokens
of context. It does its task. It closes the session. **All the context
is lost.** At the next session, it starts from scratch — re-grep,
re-read, re-30k tokens. If the previous session's decision had subtle
architectural implications, the agent doesn't retain them. If you locked
a convention ("never hit the DB directly from the handler, go through
the repository pattern"), the agent has to rediscover that rule on every
task.

This is **cognitively non-scalable**: the bigger the codebase gets, the
more each task costs in tokens, and the higher the risk of drift (the
agent taking a shortcut, inventing a call that doesn't exist, ignoring a
convention). We've seen projects where the agent ends up "no longer
usable" — not because the AI is bad, but because the understanding
system around the AI is non-existent.

### Code intelligence SaaS creates lock-in

Sourcegraph (cloud), Codesee, GitGuardian, etc. — these services index
your code in THEIR infrastructure, hand it back to you through THEIR
API, and bill you per user. If you stop paying, you lose the index. If
their company pivots, changes pricing, shuts down, gets acquired — you
lose the asset. **The graph isn't yours.** It's a service you rent. For
companies that can be OK. For open-source projects, independent
languages, indie devs who want to carry their tooling across several
machines without asking a vendor for permission — it's a regression.

---

## The system-thinking principles, applied

Five questions guide every design decision in Standardoc. They come from
a system-thinking reading of the problem, not from a marketing
checklist. They boil down to: **what survives 6 months?** If the answer
to that question is "nothing", then we shouldn't build it.

### 1. What stays stable despite the changes?

Languages mutate. TypeScript adds features every release. Rust evolves
its `async`. Vue goes from 2 to 3 then adds the composition API. Lua
already has 5 dialects (PUC, Luau, LuaJIT, …).

**What must stay stable is the IR.** A symbol is a name (FQDN), a
location (file + span), a signature, modifiers, outgoing relations
(edges). That abstraction is cross-language and cross-decade. When we
add Go in 6 months, we don't invent a new format — we extend the
frontends to produce `RawSymbol` and `RawEdge` that already exist.

→ **Decision**: a dedicated crate `standardoc-ir` that defines the
stable grammar. Everything else is a consumer.

### 2. Which choices become irreversible?

License pivots. SaaS-locked models. Proprietary DB formats. Dependencies
on a vendor's cloud services. Once committed, you don't walk back
without breaking your users.

**Standardoc minimizes irreversible traps**:

- License FSL-1.1-MIT with **automatic conversion to plain MIT** after 2
  years per release. The first converts on April 26, 2028. From then on,
  the core is legally MIT forever — whatever happens to the company, the
  maintainer, the market.
- SQLite + FTS5 — an OPEN binary format, readable with any standard
  sqlite3 tool, dumpable, migratable, versionable.
- No cloud, no auth, no phone-home telemetry. If we disappear tomorrow,
  your index keeps working.

→ **Decision**: open-source with a temporal moat guaranteed by license,
not by contract or good faith.

### 3. What creates cognitive debt?

**Everything that exists in N copies that must be kept in sync.** N
parsers of your code in N different tools. N near-identical versions of
your doc schema. N ways of representing the same relation between two
symbols. The larger N is, the more the probability of drift between the
copies tends to 1 as the code mutates.

**Standardoc collapses N → 1 on indexing.** The graph is computed once
(per code revision), stored once, consumed by as many surfaces as your
workflow demands. When the code mutates, the watcher re-indexes the
delta, and all surfaces see the new state simultaneously.

→ **Decision**: one shared graph exposed by several daemons, not several
graphs synced by hand.

### 4. What breaks at scale?

Heuristics. Regex. Naming conventions you assume without being able to
verify them. Everything that works on 100 files and crashes on 10,000.

**Standardoc uses native AST parsers**: `syn` for Rust, `swc` for TS /
JS / JSX / TSX (React included), `full_moon` for Lua, custom SFC parsers
for Vue and Svelte. No regex to extract FQDNs. No heuristic to guess
whether an identifier is a function or a type. If the AST says so, we
have the truth; otherwise, we don't know it and we admit it (`Unresolved
{ name }`).

→ **Decision**: direct AST, never regex. When a symbol can't be
resolved, we mark it `Unresolved` rather than guessing — the downstream
agent can decide what to do with it.

### 5. What becomes incomprehensible in 6 months?

AI agents that shortcut their protocol. Chat sessions with no persistent
trace of decisions. Caches that don't invalidate correctly when the code
mutates.

**Standardoc enforces an observable discipline**:

- **MCP-first guardrail**: before an agent can `Bash` / `Read` / `Grep`
  / `Glob` on your codebase, it MUST have called a Standardoc MCP tool
  in the current session. The PreToolUse hook blocks, the SessionStart
  hook resets the sentinel on every new chat. Result: the agent can't
  degenerate into a grep-loop out of laziness.
- **`current_revision()` + `check_stale()`**: the agent can verify
  whether its knowledge of a symbol is still fresh, or whether the
  watcher has re-indexed something since. No more "I read this function
  3 minutes ago but it changed in the meantime".

→ **Decision**: the discipline is encoded in the system, not in the
user's good intentions.

---

## What Standardoc is NOT

To avoid lazy comparisons:

- **It's not a local Sourcegraph.** Sourcegraph is a shared full-text +
  symbol search engine for teams, with a product focus on code review
  collaboration. Standardoc is a multi-surface semantic indexing
  infrastructure, focused on AI agents + multi-frontend. The two can
  coexist on the same monorepo and don't address the same problem.

- **It's not another LSP.** Standardoc EXPOSES LSP as one of its
  consumption surfaces, but under the hood it's a global graph, not a
  per-language server. The VSCode extension wraps the LSP daemon,
  standard LSP clients can connect to it — but the real value is in the
  graph + MCP, not in the isolated LSP.

- **It's not an AI agent.** Standardoc provides the infrastructure an AI
  agent consumes. The agent stays Claude / Cursor / Continue / Cody /
  whatever-you-want. Standardoc doesn't decide in the agent's place, it
  just gives it a structured substrate so it doesn't have to grep.

- **It's not a doc generator (yet).** The doc rendering layer
  (`@standardoc/react`, Nextra/Docusaurus adapters) arrives in beta.3.
  The graph is ready to serve it today; the rendering just isn't written
  yet.

- **It's not a hosted service.** No cloud, no SaaS, no telemetry.
  Everything lives in `.standardoc/` on your machine, gitignored,
  reproducible. If one day a complementary hosted service emerges (doc
  UI, navigation dashboard), it will be optional and the core will stay
  permanently open-source.

- **It's not a substitute for the dev.** A co-work tool is powerful for
  a dev who already understands their system. For a dev who doesn't
  master their codebase, no AI and no graph will be enough to
  compensate. Standardoc is an amplifier, not a replacement.

- **It's not an auto-magic system.** Standardoc encodes what can be
  encoded — an auto-generated skill template that teaches the agent the
  usage logic, MCP-first hooks. But **coupling an agent to these
  conventions, architecting your project with a minimum of coherence, and
  knowing when to tell the agent to use which tool** stays the operator's
  responsibility. Not all agents consume the protocol the same way: the
  calibration is tripartite (infra + agent + operator). See [test
  feedback](test-feedback.md).

---

## Construction ethics

**Craft before promises.** Nothing ships before it works locally, has
tests, and integrates cleanly. We don't announce a feature for buzz before
it's taking shape. And we dogfood — Standardoc uses Standardoc to
understand itself; if it isn't useful to us, it's useful to no one.

It's not for everyone, and that's fine: on a 5,000-line SPA, `ripgrep` +
your IDE are enough. Standardoc's value is strong on large, complex
codebases, **overkill elsewhere**, and we admit it.

---

## Going further

- **[Short-term vision →](vision-short-term.md)** — what ships in beta.2
  and 1.0
- **[Mid-term vision →](vision-mid-term.md)** — beta.3 (doc rendering,
  CLI self-management) and 1.x
- **[Long-term vision →](vision-long-term.md)** — UST + Lua plugin
  layer, post-1.0 platform
- **[Notes →](notes.md)** — observations, locked decisions, dogfood
  learnings
- **[Test feedback →](test-feedback.md)** — what we tested, what worked,
  what we dropped

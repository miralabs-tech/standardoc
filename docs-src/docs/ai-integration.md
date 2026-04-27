# AI integration — best practices

📖 English · [Français](ai-integration.fr.md)

This guide explains how to get the most out of Standardoc with AI coding assistants : **why MCP-first matters**, how to **keep long threads productive**, ready-to-use **system prompt templates**, and **per-IDE installation** for Claude Code, Cursor, Zed, Continue, and any other MCP-aware host.

---

## Why MCP-first matters

When an AI agent answers "what does function `X` take as input?", it has two paths :

1. **Without MCP** — `grep -r "fn X"`, then `cat src/foo.rs`, then guess. Cost : **30k–100k tokens** per question (full file reads, false-positive grep noise, backtracking).
2. **With Standardoc MCP** — call `get_doc("module.X")`. Cost : **~100 tokens**. Returns the canonical signature, parameter types, return type, description, and tags directly.

That's a **100x to 1000x token reduction**, on every question, every conversation, every project.

Beyond cost, MCP-first changes accuracy :
- The agent reads the **current** signature, not what it remembers from a stale grep
- Cross-references (`find_usages`, `find_implementations`, `get_type_hierarchy`) replace fuzzy reasoning
- Diagnostics (`list_diagnostics`, `validate_doc_syntax`) catch the agent's own mistakes before they ship

The catch : agents don't naturally choose MCP first. Without explicit instruction, most will fall back to grep + read habits trained on years of pre-MCP data. That's where the **system prompt templates** below come in.

> **Day-1 utility on a fork.** Even if no human ever wrote `@doc` on the
> codebase, Standardoc's virtual-annotation pass synthesizes virtual
> `@doc`/`@param`/`@returns` content from naming conventions, type
> signatures, and module structure — `get_doc("module.X")` returns useful
> descriptions on the very first scan. See "Virtual annotations" in the
> README for the full set of heuristics.

---

## Long thread hygiene — the checkpoint pattern

LLM context windows expand each year, but **prompt caching** has a hard 5-minute TTL and **agent attention drifts** with conversation length. Past ~20 substantive exchanges, you typically observe :

- The agent forgets earlier locked decisions ("are we using TypeScript or Rust again?")
- It re-explains things you already discussed
- Cost per turn climbs even when you cache
- Subtle hallucinations creep in (signatures it "remembers" wrong)

The fix is **explicit checkpointing** — at ~20 exchanges, write a `SESSION-CHECKPOINT.md` at the project root summarizing :

```markdown
# Session checkpoint — YYYY-MM-DD

## Shipped this session
- (concrete features, files, decisions)

## Current state
- (what works, what's pending, build status)

## Locked decisions
- (architecture choices that should NOT be re-litigated)

## TODO (next session)
1. (next concrete steps)
```

Then start a fresh conversation with **that file as the only context**. The new thread starts with the same shared knowledge but with full attention budget.

This pattern is documented in the user's [global Claude Code instructions](https://github.com/miralabs-tech/standardoc) and works equally well in Cursor, Zed, Continue. The Standardoc Pro UI will eventually surface a "checkpoint suggestion" UI when it detects long threads — until then, the discipline is on you.

---

## System prompt templates

Two flavors. Pick based on how strictly you want to enforce MCP-first.

### Normal — recommended default (Cursor, Zed, Continue, …)

```
# MCP/LSP First (Normal)

## Objective
Use MCP/LSP as the default path for code understanding and navigation.

## Default behavior
Before using raw file search/read:
1. Use MCP/LSP tools first for:
   - symbol discovery
   - definition lookup
   - references/usages
   - diagnostics
   - high-level architecture mapping
2. Prefer semantic/symbolic results over text grep when both are possible.

## Fallback policy
Use non-MCP exploration only if:
- MCP/LSP cannot answer, or
- MCP data is incomplete/outdated for the requested target.

When falling back, briefly state:
- why MCP was insufficient
- which fallback method is used
- what result is expected

## Editing policy
For code changes:
- Use MCP/LSP first to identify the exact symbols/files to edit.
- Then apply minimal file edits.
- Re-check impact via MCP/LSP diagnostics/references when available.
```

### Strict — recommended for Cursor & Claude Code on serious work

```
# MCP/LSP First (Strict)

## Hard rule
For discovery/analysis tasks, MCP/LSP MUST be used first.
Do not start with raw file search tools.

## Mandatory sequence
1) MCP/LSP discovery
2) MCP/LSP symbol/reference/diagnostic checks
3) Only then, if needed, file-level fallback
4) Edit
5) MCP/LSP re-validation

## Allowed fallback (exception only)
Fallback to non-MCP exploration is allowed only if:
- MCP/LSP has no capability for the task, or
- MCP/LSP returns insufficient/noisy data that blocks progress.

Before fallback, explicitly state:
- "MCP/LSP insufficient because: <reason>"
- "Fallback method: <method>"
- "Scope: <minimal scope>"

## If user says "MCP only"
Use MCP/LSP exclusively for analysis.
File tools may be used only for final patch application.
```

The key win in this template is the **transparent fallback** : the agent can't silently give up on MCP and grep — it has to declare *why* MCP fell short and what it's doing instead. That gives you, the user, a feedback loop :
- If you see the same fallback reason repeat ("MCP didn't return descriptions for X"), it's a real signal that you're missing `@doc` annotations on those symbols — fix the data, not the prompt.
- If the agent fallbacks for legitimate reasons (e.g. needs to read a `.md` page that's not indexed), you understand why instead of assuming the agent is being lazy.

Use the **Strict** template when :
- The codebase is well-indexed by Standardoc (most public symbols annotated)
- You're doing implementation work where wrong info costs real time
- You want CI-grade discipline + a visible audit trail of every fallback

Use the **Normal** template when :
- The codebase is partially indexed (many symbols not yet `@doc`'d)
- You're doing exploration / brainstorming where flexibility helps
- You don't want the agent to stall on transparent-fallback ceremony

---

## Per-IDE setup

Setup is the same `.mcp.json` snippet everywhere — only the location and discovery mechanism differ.

### Claude Code

**Per-project** — drop `.mcp.json` at your workspace root :

```json
{
  "mcpServers": {
    "standardoc": {
      "type": "stdio",
      "command": "/absolute/path/to/standardoc-server",
      "args": ["--mcp", "--workspace", "${workspaceFolder}"]
    }
  }
}
```

Restart Claude Code (or run `/mcp` to re-discover) and Standardoc's MCP tools become available.

**Global system prompt** — add the Strict template to your global `~/.claude/CLAUDE.md` :

```markdown
## Tool Hierarchy (mandatory)

When exploring or understanding code, always follow this order:

1. **MCP tools** — primary source of truth (indexed, structured, fast)
2. **LSP tools** — symbol resolution, definitions, references
3. **Read / Grep** — last resort only, and only when MCP/LSP return nothing

Never reach for `Read` or `Grep` on source files when an MCP tool can answer
the question. If MCP returns no result, say so explicitly before falling back.
```

**Long thread checkpoint** — add to the same `CLAUDE.md` :

```markdown
## Long Thread Management

When a conversation reaches ~20 significant exchanges, write a
`SESSION-CHECKPOINT.md` at the project root summarizing what was shipped,
current state, locked decisions, and what remains. Then suggest starting a
new thread with that file as the only context.
```

### Cursor

**Per-project** — same `.mcp.json` at workspace root works as of Cursor 0.42+.

**Project rules** — drop `.cursorrules` at workspace root (Cursor reads this on every chat). Either template works; **Strict is recommended as soon as the daemon is running and the index is populated** — which happens at boot, even before any `@doc` annotation exists (the AST already gives you signatures, params, return types ; annotations enrich the payload but aren't a prerequisite). The transparent-fallback statements then give you a useful signal when MCP isn't enough — typically when descriptions are missing because a symbol hasn't been annotated yet.

```
# Project rules

When exploring or modifying code, prefer MCP tools over raw file reads :
[paste the Normal or Strict template here]
```

### Zed

In `~/.config/zed/settings.json` (or workspace-level `.zed/settings.json`) :

```json
{
  "context_servers": {
    "standardoc": {
      "command": {
        "path": "/absolute/path/to/standardoc-server",
        "args": ["--mcp", "--workspace", "/absolute/path/to/your/project"]
      }
    }
  }
}
```

The Standardoc context server appears in the Assistant panel as `@standardoc`.

### Continue

Edit `~/.continue/config.json` :

```json
{
  "experimental": {
    "modelContextProtocolServers": [
      {
        "transport": {
          "type": "stdio",
          "command": "/absolute/path/to/standardoc-server",
          "args": ["--mcp", "--workspace", "/absolute/path/to/your/project"]
        }
      }
    ]
  }
}
```

### Generic MCP host

Any MCP 1.0-compatible client works — Standardoc is a vanilla stdio server. Point your client at :

- **command** : `standardoc-server` (or full path if not on `$PATH`)
- **args** : `["--mcp", "--workspace", "<absolute-path-to-project>"]`
- **transport** : stdio (JSON-RPC 2.0)

The protocol version is `2025-06-18`. Tools auto-discover via the standard `tools/list` MCP method. See the [MCP reference](mcp-reference.md) for the full surface.

---

## Verification — is MCP wired correctly?

Once configured, ask the agent : **"List the documentation blocks in this workspace using the standardoc MCP."** A working setup returns a structured list. A broken one falls back to grepping `.md` files (a tell-tale sign).

Other quick checks :
- Ask : *"What MCP tools do you see for standardoc?"* — the agent should enumerate `list_docs`, `get_doc`, `find_usages`, etc. If it says "I don't see standardoc tools", the host hasn't loaded the schemas (in Claude Code, this is the `ToolSearch` step for deferred tools — agent must call it before MCP tools are invokable).
- Ask : *"What does `standardoc-server` expose as MCP tools?"* — should call `list_docs` or similar, not read source.
- Ask : *"Find every usage of `DocBlock` in the codebase."* — should call `find_usages`, not `grep`.
- If the agent grep'd, the system prompt isn't enforcing MCP-first — re-read the **Allowed fallback** section above.

---

## Pro tip — dogfooding Standardoc on Standardoc itself

Standardoc indexes itself. If you check out [`miralabs-tech/standardoc`](https://github.com/miralabs-tech/standardoc) and point an MCP-aware agent at it, you can ask :

- *"What does `find_implementations` actually do?"* — gets the full doc of the MCP tool by querying the live index, not by reading the README.
- *"Find every place we call `scan_and_extract`."* — `find_usages` pinpoints exact locations, agent never opens a file blindly.
- *"Show me all the validator codes (STD###) and what each catches."* — `search_docs` + `get_doc` returns precise, structured info.

This is the most rigorous test of the pattern. If the agent stays MCP-first on Standardoc's own codebase (a complex Rust workspace), it'll work on yours.

# Quickstart

← [README](../../README.md) · [Roadmap](TODO-LIST.md)

From zero to a Standardoc-indexed workspace your agent can query — ~5 minutes.

---

## 1. Install

**VSCode / Cursor** — search **Standardoc** in the Marketplace or Open VSX.
On first activation it offers to download the native binary for your
platform (SHA256-verified) — accept it.

No VSCode? Skip to [§5](#5-without-vscode).

## 2. Initialize the workspace

Open a project. Standardoc asks:

> **Initialize this workspace?** — [Initialize] · [Skip] · [Never for this workspace] · [Never (any workspace)]

Click **Initialize**. It writes, idempotently:

- **`.mcp.json`** — registers Standardoc as an MCP server (HTTP, `127.0.0.1:7700`) so your agent can reach it.
- **`.claude/skills/standardoc/SKILL.md`** — teaches the agent the graph (MCP-first, the `find → context → body` flow).
- **`.claude/settings.json`** — the MCP-first hooks (see §4).

…then spawns the daemon and cold-start-indexes the workspace (a few seconds). A file watcher keeps the index live as you edit, and a status-bar item shows daemon state + the common actions.

> `.mcp.json` holds machine-absolute paths — add it to `.gitignore` if you collaborate.

## 3. `standardoc.sxd` — the workspace config

On first index, Standardoc seeds **`standardoc.sxd`** at the root (folding any legacy `.stdignore` into it, backing the old file up). It's the single source of truth for what gets indexed:

````sxd
version "0.1.0"

ignore {
  patterns ```
.git/
node_modules/
target/
dist/
```
}

# Optional. With at least one `project` block, mechanical cargo/npm/lua
# detection is skipped and ONLY these paths are indexed:
project "api" {
  label "API"
  paths ["crates/api" "crates/shared"]
}

mcp { port 7700 }   # MCP daemon port  (default 7700)
viz { port 3000 }   # graph-viz port   (default 3000)
````

Edit it freely; re-indexing picks up the changes. Blocks: `ignore`,
`project` / `group`, `mcp`, `viz`. With no `project` block, Standardoc
auto-detects cargo / npm / lua projects as before.

## 4. Use it

Ask your agent normal questions:

> *"Where is `parse_workspace` defined? Who calls it?"*

It reads the skill at boot and goes MCP-first — `find_symbol` + `get_context`
instead of grep. **~100 tokens, not 30k.** Claude Code, Cursor, Continue,
Copilot, any MCP client.

For **Claude Code**, init also installs four `.claude/settings.json` hooks
that *enforce* it:

- **UserPromptSubmit** — one-line reminder of the MCP tools.
- **PreToolUse** *(mark)* — fires on any `mcp__standardoc__*` call; marks the session.
- **PreToolUse** *(check)* — **denies** `Bash` / `Read` / `Grep` / `Glob` until the agent has used Standardoc this chat.
- **SessionStart** *(reset)* — wipes the marker so every chat starts strict.

Another agent? Wire the equivalent with `standardoc claude pre-tool-hook --mode {mark,check,reset}`.

## 5. Without VSCode

```sh
cargo install --git https://github.com/miralabs-tech/standardoc standardoc-cli
standardoc init <workspace>   # skill + MCP-first hooks + AGENTS.md + .mcp.json
```

`init` writes a `.mcp.json` that runs `standardoc mcp --connect` — a thin
bridge keeping one live, watcher-backed daemon for the workspace. Your agent
now has the graph. To drive the daemons yourself instead:

```sh
standardoc lsp <ws>                  # primary writer (holds the fs lock)
standardoc mcp <ws> --http <port>    # MCP over HTTP/SSE (multi-client)
standardoc mcp <ws> --readonly       # MCP over stdio (one client)
```

## 6. Handy sub-commands

```sh
standardoc index <ws>                   # one-shot index
standardoc rescan <ws>                  # rebuild from scratch
standardoc query <ws> ...               # CLI query (find / context / body)
standardoc sxd-preview <ws> <pattern>   # preview what .sxd ignore matches
standardoc self-update                  # update the binary in place
```

The full MCP tool set lives in the auto-generated `SKILL.md` — the agent
reads it, so you don't have to.

---

← [README](../../README.md) · [Roadmap](TODO-LIST.md)

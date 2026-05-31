# Quickstart

📖 English · [Français](../fr/QUICKSTART.md)

[Philosophy](storytelling/philosophy.md) · [Short-term vision](storytelling/vision-short-term.md) · [Notes](storytelling/notes.md) · [FAQ](FAQ.md) · [Comparison](COMPARISON.md) · [Support](SUPPORT.md)

5 minutes from zero to a Standardoc-indexed workspace, queryable by AI
agents.

---

## 1. Install the VSCode extension

Search for **Standardoc** in the VSCode / Open VSX marketplace, or grab
the VSIX from
[releases](https://github.com/miralabs-tech/standardoc/releases):

```sh
code --install-extension standardoc-X.Y.Z.vsix
```

> Just want the CLI without VSCode? Skip directly to
> [§5 — Standalone CLI](#5-without-the-vscode-extension-standalone-cli).

---

## 2. Download the `standardoc` binary

On first activation the extension shows a toast: **Standardoc needs to
download the native binary for this platform** — [Download] / [Later] /
[Show logs]. **Download** fetches the pinned `version.json`, grabs the
platform archive, verifies its SHA256, and installs the binary; **Later**
leaves a `$(cloud-download)` affordance in the status bar to retry. (The
binary ships separately from the VSIX so it can update on its own cadence.)

Once it's in place, the extension supervises the daemon and registers
Standardoc as an MCP server for Copilot Chat / Claude Code in VSCode.

> *Dev / pre-release:* set `standardoc.binaryPath` to an absolute path
> (e.g. `target/debug/standardoc`) — it always wins over the
> auto-downloaded binary.

---

## 3. Initialize a workspace

Open a project (Rust / TypeScript / JavaScript / React (JSX & TSX) / Vue
/ Svelte / Lua). A 4-button notification on first activation:

> **Standardoc: Initialize this workspace?**
> [Initialize] [Skip] [Never for this workspace] [Never (any workspace)]

Click **Initialize**. The extension:

1. Creates `.standardoc/` — SQLite index + metadata
2. Writes `.mcp.json` at the root (cross-client merge, preserves
   existing user fields)
3. Generates `.claude/skills/standardoc/SKILL.md` (teaches MCP-first,
   the 3-phase protocol, edge kinds, recommended workflows)
4. Spawns the LSP daemon (primary writer) + the MCP HTTP/SSE daemon, then
   cold-start indexes your workspace (5–15s depending on size, progress
   visible via `$/progress` on the LSP side)

The watcher keeps the index live after the cold start. Every
modification of `*.rs` / `*.ts` / `*.tsx` / `*.js` / `*.jsx` / `*.vue` /
`*.svelte` / `*.lua` triggers an incremental re-index.

A **Standardoc item appears in the status bar** (right corner of the
VSCode window) — it shows the daemon's state in real time and serves as
the entry point to all common actions.

> Consider adding `.mcp.json` to your `.gitignore` if you collaborate —
> the file contains absolute machine paths that aren't portable.

---

## 4. Use it

### With an AI agent

Claude Code, Cursor, Continue, Copilot Chat, Aider, Goose, Cody, any
MCP-aware client. Ask normal questions about your codebase. The agent
reads the auto-generated skill at boot and switches to MCP-first mode:

> *"Where is `parse_workspace` defined? Who calls it? What enrichments
> are attached to it?"*

The agent uses `find_symbol` + `get_context` (depth=1 first, depth=2
after) instead of grep. **~100 tokens per question instead of 30k.**

### Claude Code hooks installed automatically

The opt-in init flow installs the **MCP-first guardrail** into the
workspace's `.claude/settings.json` — it stops the agent from
degenerating into a grep loop. Three coordinated hooks:

- `SessionStart` *(reset)* — wipes the sentinel on every new chat to
  stay strict
- `PreToolUse` *(mark)* — places the sentinel as soon as a Standardoc
  tool is called
- `PreToolUse` *(check)* — blocks `Bash` / `Read` / `Grep` / `Glob` when
  the sentinel is absent (only when the call targets a path inside the
  workspace — reads outside it, e.g. `~/.claude`, are never gated)

If you use another MCP-aware client, configure your own equivalent
hooks:

```sh
standardoc claude pre-tool-hook --mode mark   # place the sentinel
standardoc claude pre-tool-hook --mode check  # block Bash/Grep/...
standardoc claude pre-tool-hook --mode reset  # wipe on SessionStart
```

### The status bar menu (click bottom-right)

The Standardoc item in the status bar opens a QuickPick with **the
common actions**:

- **▶ Start daemon** / **■ Stop daemon** / **↻ Restart daemon**
- **🗑 Purge excluded paths** — purges the symbols whose source file
  now matches `.stdignore`

### VSCode command palette (`Ctrl+Shift+P`)

All status-bar actions are reachable via `Standardoc: …`, plus palette-only
commands: **Find symbol**, **Get context for symbol at cursor**,
**Initialize workspace**, **Refresh .mcp.json paths**, **Regenerate AI
agent skill**, **Reset global init prompt**.

### Verify the agent sees Standardoc

Your MCP client lists connected servers — Standardoc should appear with its
tools (`find_symbol`, `get_context`, `get_body`, `find_call_sites`,
`fetch_graph`, …). If it doesn't: check the status-bar item (daemon
running? else **Restart daemon**), re-run `Standardoc: Refresh .mcp.json
paths` if the workspace moved, and read the `Standardoc` output channel for
startup errors.

---

## 5. Without the VSCode extension (standalone CLI)

Standardoc works with any MCP-aware client, with no VSCode dependency.

### Install the binary

**Pre-built binaries** (primary channel) — download the archive
matching your platform from
[releases/latest](https://github.com/miralabs-tech/standardoc/releases/latest).
The `version.json` manifest lists the archives per platform with
SHA256 for verification.

**OR via cargo** (source build):

```sh
cargo install --git https://github.com/miralabs-tech/standardoc standardoc-cli
standardoc --version
```

### Launch the daemons

```sh
# Primary writer (acquires the fs lock on .standardoc/)
standardoc lsp /abs/path/to/workspace

# MCP read-only, stdio transport (one client at a time)
standardoc mcp /abs/path/to/workspace --readonly

# MCP read-only, HTTP/SSE transport (multi-client)
standardoc mcp /abs/path/to/workspace --readonly --http 0
# Endpoint URL written to .standardoc/mcp.endpoint
```

### Minimal MCP config (stdio client)

```json
{
  "mcpServers": {
    "standardoc": {
      "type": "stdio",
      "command": "/abs/path/to/standardoc",
      "args": ["mcp", "/abs/path/to/workspace", "--readonly"]
    }
  }
}
```

### Common config locations

- **Claude Desktop** — `claude_desktop_config.json`
  (Settings → Developer → Edit Config)
- **Claude Code CLI** — `~/.claude.json` or per-project `.mcp.json`
- **Cursor** — `~/.cursor/mcp.json` or `.cursor/mcp.json` (workspace)
- **Other MCP-aware clients** — see their respective docs

Several `--readonly` MCP clients can attach concurrently to the same
SQLite index without contention. The LSP holds the workspace fs lock as
the primary writer.

---

## 6. Useful sub-commands

```sh
standardoc init <ws>                   # install skill + hooks + AGENTS.md + .mcp.json
standardoc lsp <ws>                    # primary writer daemon
standardoc mcp <ws> --connect          # stdio↔http bridge (what init wires into .mcp.json)
standardoc mcp <ws> --readonly         # readonly MCP daemon (stdio)
standardoc mcp <ws> --http <port>      # MCP daemon (HTTP/SSE)
standardoc index <ws>                  # one-shot index
standardoc watch <ws>                  # watcher only
standardoc rescan <ws>                 # rebuild from scratch
standardoc query <ws> ...              # CLI query (find / context / body)
standardoc purge-excluded <ws>         # cleanup post-.stdignore edit
standardoc schema-version <ws>         # print schema version
standardoc sxd-preview <ws> <pattern>        # preview .stdignore matches
```

The MCP surface exposed by the daemon (`find_symbol`, `get_context`,
`get_body`, `get_code`, `find_call_sites`, `module_lookup`, `fetch_graph`,
`current_revision`, `check_stale`, plus the cross-workspace
`link_workspace` / `resolve_cross_workspace` family) is documented in
detail in the auto-generated `SKILL.md` at workspace init — that's what
the agent reads to know how to use Standardoc. No need to memorize it on
the human side.

---

## 7. Tune the index

### `.stdignore`

Auto-seeded at the workspace root on first init. **Gitignore syntax**,
hot-reload of changes.

Default template: `.git/`, `node_modules/`, `target/`, `dist/`,
`build/`, `.old/`, `*-old/`, `test-export/`.

- **Additions** → exclude paths (automatic purge of matching symbols
  via `standardoc purge-excluded`)
- **Removals** → automatic re-index of the affected sub-tree

---

## What's next

- **[Philosophy](storytelling/philosophy.md)** — the 5 system-thinking
  principles and the construction ethics
- **[Short-term vision](storytelling/vision-short-term.md)** — beta.2
  and the stabilization phase
- **[Mid-term vision](storytelling/vision-mid-term.md)** — beta.3 and
  1.0
- **[Notes](storytelling/notes.md)** — dogfood observations, posture,
  support
- **[FAQ](FAQ.md)** — common questions
- **[Comparison](COMPARISON.md)** — vs LSP / Sourcegraph / others
- **[Support](SUPPORT.md)** — how to support the project

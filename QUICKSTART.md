# Quickstart

📖 English · [Français](QUICKSTART.fr.md)

5 minutes from zero to a Standardoc-indexed workspace that AI agents can query.

---

## 1. Install the CLI

```sh
cargo install standardoc-cli
standardoc --version
```

You now have a single binary `standardoc` with sub-commands:

```sh
standardoc lsp <workspace>             # primary writer daemon (acquires fs lock)
standardoc mcp <workspace> --readonly  # read-only MCP daemon
standardoc index <workspace>           # one-shot scan + index
standardoc rescan <workspace>          # rebuild from scratch
standardoc purge-excluded <workspace>  # drop symbols matching .stdignore
```

You can stop here if you only want CLI / standalone MCP usage with Claude
Desktop, Cursor or the Claude Code CLI — see [§5](#5-use-without-the-vscode-extension).
For the integrated VSCode flow, continue.

---

## 2. Install the VSCode extension

Search **Standardoc** in the VSCode Extensions panel, or grab the latest VSIX
from [releases](https://github.com/miralabs-tech/standardoc/releases) and:

```sh
code --install-extension standardoc-X.Y.Z.vsix
```

The extension auto-spawns the daemon, supervises restarts, registers Standardoc
as an MCP server for Copilot Chat / Claude Code in VSCode, and surfaces a
status bar item.

---

## 3. Initialize a workspace

Open any Rust or TypeScript project in VSCode. On first activation you get a
notification:

> **Standardoc: Initialize this workspace?** (DB index + register MCP for Claude Code CLI)
>
> [Initialize] [Skip] [Never for this workspace] [Never (any workspace)]

Click **Initialize**. The extension:

1. Creates `.standardoc/` (SQLite index + workspace metadata)
2. Writes `.mcp.json` at the workspace root with absolute paths
3. Generates `.claude/skills/standardoc/SKILL.md` for Claude Code
4. Spawns the LSP daemon (cold start ~5-15s on first run)

Cold start indexes every `.rs` / `.ts` / `.tsx` / `.js` / `.jsx` file. After
that, a watcher keeps the index live.

> Consider adding `.mcp.json` to your `.gitignore` if collaborating —
> the file contains machine-absolute paths that aren't portable.

---

## 4. Use it

### From AI agents (Claude Code / Copilot Chat in VSCode)

The skill auto-loads. Just ask the agent normal questions about your codebase:

> *"Where is `parse_workspace` defined? Who calls it?"*

The agent uses `find_symbol` + `get_context` instead of grepping. ~100 tokens
per query instead of 30k.

### From the VSCode command palette

- `Standardoc: Find symbol` — InputBox + QuickPick over `find_symbol`, opens
  the chosen symbol at its source location.
- `Standardoc: Get context for symbol at cursor` — runs `find_symbol` on the
  word under the cursor, takes the top match, renders `get_context(fqdn,
  depth=1)` into the Standardoc output channel.
- `Standardoc: Daemon: Stop` / `Start` / `Restart`
- `Standardoc: Refresh .mcp.json paths` — re-merge with current absolute paths
  after moving the workspace or rebuilding the binary elsewhere.
- `Standardoc: Regenerate AI agent skill` — overwrite the SKILL.md template.

---

## 5. Use without the VSCode extension

Standardoc MCP works with any MCP-aware client. Add this to your client's MCP
config (replace `<workspace>` and `<binary>` with absolute paths):

```json
{
  "mcpServers": {
    "standardoc": {
      "type": "stdio",
      "command": "<binary>",
      "args": ["mcp", "<workspace>", "--readonly"]
    }
  }
}
```

You also need an active LSP daemon (the **primary writer**) to keep the index
fresh. Run it in a terminal:

```sh
standardoc lsp /abs/path/to/workspace
```

The LSP holds the workspace fs lock; multiple `--readonly` MCP clients can
attach concurrently to the same SQLite index without contention.

Common config locations:

- **Claude Desktop** — `claude_desktop_config.json` (Settings → Developer → Edit Config)
- **Claude Code CLI** — `~/.claude.json` or per-project `.mcp.json`
- **Cursor** — `~/.cursor/mcp.json` or workspace `.cursor/mcp.json`

---

## 6. Tune the index

### `.stdignore`

Auto-seeded at workspace root on first init. Gitignore syntax. Default
template excludes `.git/`, `node_modules/`, `target/`, `dist/`, `build/`,
`.old/`, `*-old/`, `test-export/`. Edit freely — additions exclude paths,
removals trigger an auto re-index of the affected subtree.

### Pause / purge

- `standardoc purge-excluded <workspace>` removes from the index any symbol whose
  source file now matches `.stdignore` (useful after enriching the file).

---

## Where to next

- [README.md](README.md) — full feature surface and architecture diagram
- [ABOUT.md](ABOUT.md) — why Standardoc exists and how it differs from LSP /
  Sourcegraph / TypeDoc / etc.
- [FAQ.md](FAQ.md) — common questions
- [COMPARISON.md](COMPARISON.md) — side-by-side with adjacent tools

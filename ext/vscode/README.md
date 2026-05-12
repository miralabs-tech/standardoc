# Standardoc — VSCode extension

AI-readable code documentation indexer for Rust and TypeScript projects, surfaced
over LSP (writer daemon) and MCP (read-only daemon) via stdio.

## Architecture

The extension spawns the bundled `standardoc` binary as two parallel daemons:

- `standardoc lsp <workspace>` — primary writer (acquires the workspace fs lock,
  runs the indexer, serves LSP requests).
- `standardoc mcp <workspace> --readonly` — read-only client over a separate
  SQLite connection, exposes MCP tools (`get_context`, `find_symbol`).

## Settings

- `standardoc.binaryPath` — absolute path override. When unset the extension
  falls back to the binary bundled under `dist/bin/`, then to a `standardoc`
  found on `PATH`.

## Commands

- `Standardoc: Daemon: Restart` / `Daemon: Stop` / `Daemon: Start`
- `Standardoc: Find symbol` — InputBox + QuickPick over `find_symbol`, opens the
  selected symbol at its source location.
- `Standardoc: Get context for symbol at cursor` — runs `find_symbol` on the
  word under the cursor, takes the top match, and renders
  `get_context(fqdn, depth=1)` into the Standardoc output channel.
- `Standardoc: Initialize for this workspace` — manual override of the init
  prompt. Writes `.mcp.json` and `.claude/skills/standardoc/SKILL.md`, opts
  this workspace in, and starts the daemons.
- `Standardoc: Refresh .mcp.json paths` — re-merge `.mcp.json` with current
  absolute paths (use after moving the workspace or rebuilding the binary
  elsewhere).
- `Standardoc: Regenerate AI agent skill` — overwrite
  `.claude/skills/standardoc/SKILL.md` with the latest template.
- `Standardoc: Reset global init prompt` — clear the "Never (any workspace)"
  global opt-out so the init prompt re-appears on workspaces with code.
- `Standardoc: Purge excluded paths` — stop the daemon, run
  `standardoc purge-excluded`, restart.

## AI agent skill (Claude Code)

When you opt in to initialization on a workspace, the extension generates a
skill file at `.claude/skills/standardoc/SKILL.md`. Claude Code (CLI and IDE
integrations) auto-loads skills from `.claude/skills/` and uses the
`description` / `when_to_use` frontmatter to decide when to invoke them.

The generated skill instructs the AI to use Standardoc as the **first** tool
for any code-understanding task on this workspace, falling back to LSP /
Go-to-Definition and then to raw `Read`/`Grep`/`Glob` only when Standardoc
cannot answer. It also pre-approves the two MCP tools
(`mcp__standardoc__find_symbol`, `mcp__standardoc__get_context`) so they run
without per-call permission prompts.

If you edit the file by hand, re-running `Standardoc: Regenerate AI agent
skill` will overwrite your changes. The init flow leaves an existing skill
file untouched (only writes when absent or already matching the template).

## Use Standardoc MCP from external chat clients

The extension contributes Standardoc as an MCP server provider for the editor's
own chat surface (Copilot Chat, Claude Code in VSCode, …). On activation it
registers under id `standardoc.mcp` and the editor picks up the `find_symbol`
and `get_context` tools automatically — no extra config inside the editor.

To use Standardoc MCP from chat clients **outside** VSCode (Claude Desktop,
Cursor, the standalone Claude Code CLI, …) add the following entry to the
client's MCP config file. Replace `<workspace>` with the absolute path to your
project root and `<binary>` with the absolute path to your `standardoc` build
(or `standardoc` if it is on your `PATH`):

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

> If you collaborate via git, consider adding `.mcp.json` to your `.gitignore` —
> the file contains machine-absolute paths that are not portable across teammates.

Common file locations:

- Claude Desktop — `claude_desktop_config.json` (Settings → Developer → Edit Config)
- Claude Code CLI — `~/.claude.json` or per-project `.mcp.json`
- Cursor — `~/.cursor/mcp.json` or workspace `.cursor/mcp.json`

The `--readonly` flag lets multiple chat clients share the index concurrently
with the LSP daemon owned by the VSCode extension.

## Local development (dogfood)

```sh
bun install
# Point standardoc.binaryPath in VSCode settings to
# <repo>/standardoc/target/debug/standardoc(.exe), then:
bun run package
bun run dev:install
# Reload the VSCode window and exercise the extension live.
```

## License

[FSL-1.1-MIT](./LICENSE) — Functional Source License with MIT Future License.
Copyright 2026 Wesley Cormier (miralabs.tech).

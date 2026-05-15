# Standardoc — VSCode extension

> A code intelligence infrastructure, built on a canonical multi-language
> IR and a living semantic graph. One graph, several daemons (LSP, MCP
> stdio + HTTP/SSE), all your tools plugged into it. ~100 tokens per
> agent query instead of 30k of grep + read. Local, derived from source
> code, open-source.
>
> Built for what breaks at scale and turns unmanageable in 6 months, not
> for the 2-minute demo. Code understanding is a system, not a string of
> greps.

## 🆕 v1.1.0 — breaking install change

**The `standardoc` binary is no longer bundled in the VSIX.**

On first activation the extension transitions into a new `awaiting_binary` state and surfaces a one-time toast:

> **Standardoc needs to download the native binary for this platform.**
> &nbsp; [Download] &nbsp; [Later] &nbsp; [Show logs]

- **Download** → the extension fetches the pinned `version.json` manifest from the matching GitHub Release, downloads the platform archive (`.tar.gz` on Linux/macOS, `.zip` on Windows), verifies its SHA256, extracts it with the system `tar`, and installs the binary under `<globalStorageUri>/bin/<rust-target-triple>/standardoc[.exe]`.
- **Later** → the daemon stays in `awaiting_binary`. The status bar shows a `$(cloud-download) Standardoc` affordance; one click re-triggers the download.

**Why** the VSIX would be heavy (tens of MB × N platforms) and the binary evolves at a pace independent of the ext release cycle. The decoupling lets us ship binary updates without bumping the extension. Compat check rides on the SHA256: a matching SHA proves the binary published under the pinned release tag.

**Override** Set `standardoc.binaryPath` to an absolute path if you want to point at a local build (`target/debug/standardoc`) or a pre-release binary you want to test against the current extension. The setting always wins over the auto-downloaded binary.

## Architecture

The extension supervises the `standardoc` binary as two parallel daemons:

- `standardoc lsp <workspace>` — primary writer (acquires the workspace fs lock, runs the indexer, serves LSP requests).
- `standardoc mcp <workspace> --readonly --http <port>` — read-only over a separate SQLite connection, exposes MCP tools over streamable HTTP/SSE. Stdio transport is also available for non-VSCode clients.

A daemon supervisor handles state (`stopped`, `starting`, `ready`, `restarting`, `failed`, `fatal_config`, `awaiting_binary`), parallel spawn with `Promise.allSettled` rollback, backoff state machine, and structured `STDOC_FATAL` markers for unrecoverable host-side conditions.

## Install

See the [QUICKSTART](https://github.com/miralabs-tech/standardoc/blob/main/.important/en/QUICKSTART.md) on the official GitHub repo for the full 5-minute path:

1. Install the extension (this page).
2. On first activation → click `Download` on the binary toast.
3. Open a project → click `Initialize` on the workspace init prompt.
4. Standardoc takes over indexing in the background; status bar shows daemon state in real time.

## Settings

- **`standardoc.binaryPath`** — absolute path override. Takes priority over the auto-downloaded binary. Useful for local dev (`target/debug/standardoc`) and pre-release testers.
- **`standardoc.mcpHttpPort`** — TCP port on `127.0.0.1` (default `7700`). Set to `0` for an ephemeral kernel-picked port (resolved URL is written to `.standardoc/mcp.endpoint`).
- **`standardoc.ragEnabled`** — enable the RAG (prose retrieval) layer. Adds `chunk_refs` to `get_context` responses and exposes the `fetch_chunks` MCP tool. Daemon restarts automatically when this changes.
- **`standardoc.ragEmbedder`** — `mock` (deterministic zero-network stub) or `candle` (local BGE-small-en-v1.5, ~130 MB downloaded on first use to `~/.cache/standardoc/models/`).

## Commands

Daemon

- `Standardoc: Daemon: Start` / `Stop` / `Restart`
- `Standardoc: Download binary` — fetch + SHA-verify + install the platform binary into globalStorage.

Workspace

- `Standardoc: Initialize for this workspace` — writes `.mcp.json` + `.claude/skills/standardoc/SKILL.md`, opts the workspace in.
- `Standardoc: Refresh .mcp.json paths` — re-merge `.mcp.json` with current absolute paths.
- `Standardoc: Regenerate AI agent skill` — overwrite the generated SKILL.md with the latest template.
- `Standardoc: Reset global init prompt` — clear the global "Never (any workspace)" opt-out.
- `Standardoc: Purge excluded paths` — stop daemon, run `standardoc purge-excluded`, restart.

Symbols

- `Standardoc: Find symbol` — InputBox + QuickPick over `find_symbol`, opens the selected symbol at its source location.
- `Standardoc: Get context for symbol at cursor` — runs `find_symbol` on the word under the cursor, takes the top match, renders `get_context(fqdn, depth=1)` into the output channel.

RAG (prose retrieval)

- `Standardoc: Toggle RAG (prose retrieval)`
- `Standardoc: Switch RAG embedder…`
- `Standardoc: Rebuild RAG index`

Telemetry-free token savings

- `Standardoc: Show token savings`
- `Standardoc: Reset token savings…`

## AI agent skill (Claude Code)

When you opt in to initialization, the extension generates a skill file at `.claude/skills/standardoc/SKILL.md`. Claude Code (CLI and IDE integrations) auto-loads skills from `.claude/skills/` and uses the `description` / `when_to_use` frontmatter to decide when to invoke them.

The generated skill teaches the agent the **3-phase protocol** (`find_symbol` → `get_context` (depth=1) → `get_body` or depth=2) and pre-approves every shipped MCP tool so they run without per-call permission prompts: `find_symbol`, `find_symbols_by_pattern`, `find_similar_symbols`, `get_context`, `get_body`, `list_symbols`, `current_revision`, `check_stale`, `fetch_chunks`, `resolve_external`, `session_*`, `usage_stats`.

A `PreToolUse` hook denies `Bash` / `Read` / `Grep` / `Glob` until a Standardoc MCP tool has been called in the session — this is what the skill calls **MCP-first** and what unlocks the ~100-tokens-per-query payoff. A `SessionStart` hook wipes the sentinel each new chat.

## MCP from external chat clients

The extension contributes Standardoc as an MCP server provider for the editor's own chat surface (Copilot Chat, Claude Code in VSCode, …). No extra config inside the editor.

To use Standardoc MCP from chat clients **outside** VSCode (Claude Desktop, Cursor, the standalone Claude Code CLI, …), pick stdio or HTTP/SSE:

**stdio**

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

**HTTP** (already exposed by the ext on `mcpHttpPort`, default `7700`)

```json
{
  "mcpServers": {
    "standardoc": {
      "type": "http",
      "url": "http://127.0.0.1:7700/mcp"
    }
  }
}
```

The init flow already writes the HTTP entry to `.mcp.json` at the workspace root, so any client that picks up that file (Claude Code CLI, Copilot Chat, …) is configured automatically — the JSON above is only useful for clients that don't read project-local `.mcp.json` or for a manual override.

If port `7700` was already bound (sibling VSCode window, port conflict), the daemon falls back to an ephemeral kernel-picked port and writes the actual URL to `.standardoc/mcp.endpoint` — the ext re-syncs `.mcp.json` to that URL on `ready`. Set a different `standardoc.mcpHttpPort` per workspace to avoid the fallback dance.

Common config file locations: Claude Desktop → `claude_desktop_config.json`; Claude Code CLI → `~/.claude.json` or per-project `.mcp.json`; Cursor → `~/.cursor/mcp.json`.

> If you collaborate via git, consider adding `.mcp.json` to your `.gitignore` — the file contains machine-absolute paths that are not portable across teammates.

## Support

- **Issues** — [github.com/miralabs-tech/standardoc/issues](https://github.com/miralabs-tech/standardoc/issues) for bugs, regressions, feature requests.
- **Discussions and roadmap** — see [SUPPORT.md](https://github.com/miralabs-tech/standardoc/blob/main/.important/en/SUPPORT.md) for triage policy, scope discipline, and current focus areas. Standardoc is maintained by a single person; the SUPPORT doc explains what is in / out of scope before 1.0.
- **Roadmap and what's shipped** — [TODO-LIST.md](https://github.com/miralabs-tech/standardoc/blob/main/.important/en/TODO-LIST.md).
- **Storytelling** (philosophy, short / mid / long-term vision) — [.important/en/storytelling/](https://github.com/miralabs-tech/standardoc/tree/main/.important/en/storytelling).

## Local development (dogfood)

```sh
# 1. Build the core binary
cargo build -p standardoc-cli            # or `cargo build --release -p standardoc-cli`

# 2. Point the extension at it via VSCode settings:
#    "standardoc.binaryPath": "<repo>/target/debug/standardoc[.exe]"

# 3. Build + install the VSIX
bun install
bun run package
bun run dev:install
# Reload the VSCode window and exercise the extension live.
```

`bun run package` no longer copies the binary into the VSIX — `standardoc.binaryPath` is the canonical dev path. The auto-downloader is bypassed entirely when the setting is set.

## License

[FSL-1.1-MIT](./LICENSE) — Functional Source License with MIT Future License. Each tagged release converts to MIT 2 years after publication. Copyright 2026 Wesley Cormier (miralabs.tech).

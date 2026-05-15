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

On first activation the extension enters `awaiting_binary` and
surfaces a toast:

> **Standardoc needs to download the native binary for this platform.**
> &nbsp; [Download] &nbsp; [Later] &nbsp; [Show logs]

- **Download** → the extension fetches `version.json` from
  `releases/download/v<BINARY_VERSION>/version.json` (pinned to the
  release this ext build expects, not `latest`), downloads the
  platform archive (`.tar.gz` on Linux/macOS, `.zip` on Windows),
  verifies the SHA256, extracts it with the system `tar`, and installs
  the binary under
  `<globalStorageUri>/bin/<rust-target-triple>/standardoc[.exe]`.
- **Later** → the daemon stays in `awaiting_binary`. The status bar
  shows a `$(cloud-download) Standardoc` affordance; one click
  re-triggers the download.

> *Why no bundled binary?* The VSIX would be heavy (tens of MB × N
> platforms), and the binary evolves at a pace independent of the ext
> release cycle. The decoupling lets us update the binary without
> bumping the extension — compat check via the `protocol_version` field
> in the `version.json` manifest.

Once the binary is in place, the extension supervises the daemon,
handles restarts (parallel spawn, `Promise.allSettled` rollback, backoff
state machine), and registers Standardoc as an MCP server for Copilot
Chat / Claude Code in VSCode.

### For developers / pre-release testers

Set `standardoc.binaryPath` to an absolute path. The setting always
takes priority over the auto-downloaded binary, so you can point at
`target/debug/standardoc` while iterating locally, or at a specific
pre-release binary to test it against the current ext.

---

## 3. Initialize a workspace

Open a project (Rust / TypeScript / JavaScript / React (JSX & TSX) / Vue
/ Svelte / Lua). A 4-button notification on first activation:

> **Standardoc: Initialize this workspace?**
> [Initialize] [Skip] [Never for this workspace] [Never (any workspace)]

Click **Initialize**. The extension:

1. Creates `.standardoc/` — SQLite index + RAG + metadata
2. Creates `.standardoc-sessions/` — cross-session agent memos
3. Writes `.mcp.json` at the root (cross-client merge, preserves
   existing user fields)
4. Generates `.claude/skills/standardoc/SKILL.md` (~480 lines —
   teaches MCP-first, the 3-phase protocol, edge kinds, 9 recommended
   workflows)
5. Spawns the LSP daemon (primary writer) + the MCP HTTP/SSE daemon
6. Cold-start indexes your workspace (5–15s depending on size, progress
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

The opt-in init flow injects **two mechanisms** into the workspace's
`.claude/settings.json`:

**1. MCP-first guardrail** — stops the agent from degenerating into a
grep loop. Three coordinated hooks:

- `SessionStart` *(reset)* — wipes the sentinel on every new chat to
  stay strict
- `PreToolUse` *(mark)* — places the sentinel as soon as a Standardoc
  tool is called
- `PreToolUse` *(check)* — blocks `Bash` / `Read` / `Grep` / `Glob` if
  the sentinel is absent

**2. Auto-sync sessions** *(`PostToolUse`)* — when the agent writes a
memo into its **Claude native memory folder**
(`~/.claude/projects/<hash>/memory/*.md`), the content is auto-imported
into `.standardoc-sessions/sessions.db`.

> **Important: the hook fires only on those writes.** Not on `Write` /
> `Edit` / `MultiEdit` of source code or project files — only files
> whose path matches `/.claude/projects/` AND `/memory/` (the
> `MEMORY_PATH_MARKER` + `MEMORY_PATH_TAIL` constants on the
> `standardoc-cli` side).

**An automatic bridge between Claude Code's native memory and
Standardoc's sessions DB** — the agent capitalizes its memos without
having to re-write them via `session_save` every time.

If you use another MCP-aware client, configure your own equivalent
hooks:

```sh
standardoc claude pre-tool-hook --mode mark   # place the sentinel
standardoc claude pre-tool-hook --mode check  # block Bash/Grep/...
standardoc claude pre-tool-hook --mode reset  # wipe on SessionStart
standardoc session hook                        # auto-import memo (PostToolUse)
```

### Persistent sessions (cross-chat memory)

`.standardoc-sessions/sessions.db` is **created automatically on the
first call to a `session_*` tool** by the agent — no human action is
required to init the DB. The DB is deliberately separate from
`.standardoc/` so that a graph reset (or a `Standardoc: Rebuild RAG
index`) doesn't wipe the memos.

**Initiating the convention stays the operator's job.** The agent
doesn't spontaneously create memos — it knows how to because the
auto-generated skill documents the workflow, but it waits to be told.
You have to **brief the agent on the first chat**:

> *"Organize yourself in sessions. Save a memo `session_save(slug,
> body_md)` at the end of each work item or locked decision. At the
> start of the next chat, do `session_get()` to recover where we
> were."*

Once the convention is established in the first chats, the agent
maintains it on its own via:

- **End of session** (locks decisions or ships work):
  `session_save(slug, body_md, supersedes?)` to persist a memo. The
  `supersedes` chains memos when a refactor invalidates a previous
  lock.
- **Start of the next session**: `session_get()` (no slug) returns the
  most recent active memo as the entry point.
- `session_list({active_only: true})` to scan recent memos.

Four distinct kinds via the frontmatter `type` field: `session`
(default handoff), `feedback` (behavioral rules), `profile` (stable
user facts), `lock` (locked decisions — **ADR equivalent** in memo
format).

**Manual migration of an external `.md` folder**:

```sh
# Import a .md folder → sessions.db
standardoc session sync-in /path/workspace /path/memos-folder

# Export sessions.db → .md folder (full frontmatter)
standardoc session sync-out /path/workspace /path/export-folder
```

### The status bar menu (click bottom-right)

The Standardoc item in the status bar opens a QuickPick with **the
common actions**:

- **▶ Start daemon** / **■ Stop daemon** / **↻ Restart daemon**
- **🗑 Purge excluded paths** — purges the symbols whose source file
  now matches `.stdignore`
- **Enable / Disable RAG** *(dynamic toggle based on state)*
- **Switch RAG embedder…** — choice between:
  - **Mock**: deterministic, zero network (for dev / tests)
  - **Candle (BGE-small)**: local 384-dim BERT. First run: ~130 MB
    download (cache `~/.cache/standardoc/models/`, override via the
    `STANDARDOC_MODELS_DIR` env variable)
- **Rebuild RAG index** — stops the daemon, deletes
  `.standardoc/rag.db` (+ `-wal` / `-shm`), restarts. The chunks are
  re-embedded at cold start. Confirmation modal before execution.
- **Show token savings** — displays the `bytes_out / baseline_bytes`
  ratio (what Standardoc returned vs. what the agent would have read
  raw) per period (today / day / week / all)
- **Reset token savings…** — baselines a clean measurement

### VSCode command palette (`Ctrl+Shift+P`)

All status-bar menu actions are reachable from the palette via
`Standardoc: …`, plus a few palette-only commands:

- `Standardoc: Find symbol` — InputBox + QuickPick on `find_symbol`,
  opens the chosen symbol at its source
- `Standardoc: Get context for symbol at cursor` —
  `get_context(depth=1)` rendered in the output channel
- `Standardoc: Initialize workspace` — re-triggers the opt-in init flow
  (useful if `.standardoc/` was deleted)
- `Standardoc: Refresh .mcp.json paths` — re-merges with the current
  absolute paths after moving the workspace or rebuilding the binary
  elsewhere
- `Standardoc: Regenerate AI agent skill` — overwrites
  `.claude/skills/standardoc/SKILL.md` (useful after an ext upgrade)
- `Standardoc: Reset global init prompt` — re-arms the 4-button
  notification even on workspaces where *Never* was already clicked

### Verify the agent sees Standardoc

On the MCP client side (Copilot Chat / Claude Code in VSCode, Cursor,
etc.), each client has its own UI to list connected MCP servers and
their available tools. Standardoc must appear there with its **16
tools** (`find_symbol`, `get_context`, `get_body`, `fetch_chunks`,
`session_save`, etc.).

If the agent says Standardoc isn't available:

1. **Status bar** — does the item indicate the daemon is running? If
   not, **Restart daemon** from the menu.
2. **`.mcp.json`** — are the absolute paths still valid (workspace
   moved, binary updated)? Run `Standardoc: Refresh .mcp.json paths`.
3. **The `Standardoc` output channel** displays the daemon and
   supervisor logs — that's where you see startup errors,
   `STDOC_FATAL` markers, embedder DLs, etc.

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
standardoc lsp <ws>                    # primary writer daemon
standardoc mcp <ws> --readonly         # readonly MCP daemon (stdio)
standardoc mcp <ws> --http <port>      # readonly MCP daemon (HTTP/SSE)
standardoc index <ws>                  # one-shot index
standardoc watch <ws>                  # watcher only
standardoc rescan <ws>                 # rebuild from scratch
standardoc query <ws> ...              # CLI query (find / context / body)
standardoc purge-excluded <ws>         # cleanup post-.stdignore edit
standardoc reset-usage --period <p>    # reset usage_stats (today/day/week/all)
standardoc schema-version <ws>         # print schema version
standardoc session sync-in <ws> <dir>  # bridge .md memos → sessions.db
standardoc session sync-out <ws> <dir> # bridge sessions.db → .md memos
standardoc stdignore-preview <ws> <pattern>  # preview .stdignore matches
```

The MCP surface exposed by the daemon (**16 tools**: `find_symbol`,
`get_context`, `get_body`, `fetch_chunks`, `session_save`,
`current_revision`, `check_stale`, `usage_stats`, etc.) is documented in
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

### RAG over the adjacent prose

`docs/`, `notes/`, and the `*.md` at the root + at sub-package roots are
chunked and reachable via the `chunk_refs` of `get_context`, or
directly via `fetch_chunks(uri)`.

The default embedder is **Candle BGE-small** (~130 MB, lazy download on
first RAG use). **Runs locally, no cloud.** The chunks live in
`.standardoc/rag.db`, linked to the graph by FQDN.

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

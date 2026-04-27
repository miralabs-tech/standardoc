# CLI reference

📖 English · [Français](cli-reference.fr.md)

Standardoc ships two binaries :

- **[`standardoc`](#standardoc-cli)** — one-shot CLI for scan / transform / emit / validate / materialize
- **[`standardoc-server`](#standardoc-server)** — long-running daemon with four transports (LSP / MCP / Web / static export)

---

## `standardoc` — CLI

```
standardoc <command> [args...]
```

Global behaviors :
- All commands take a workspace root as their first positional argument.
- The workspace is scanned with the four built-in providers (Rust / TS / Python / Lua tree-sitter) plus any dynamic provider declared in `.standardoc/languages/*.json`.
- Configuration is read from `.standardoc.json` at the workspace root if present; otherwise defaults apply.
- Output goes to **stdout**. Status, errors and counters go to **stderr** so you can pipe stdout cleanly.

### `scan`

{{ @doc.cli.commands.scan:usage }}

{{ @doc.cli.commands.scan:description }}

### `transform`

{{ @doc.cli.commands.transform:usage }}

{{ @doc.cli.commands.transform:description }}

### `emit`

{{ @doc.cli.commands.emit:usage }}

{{ @doc.cli.commands.emit:description }}

### `validate`

{{ @doc.cli.commands.validate:usage }}

{{ @doc.cli.commands.validate:description }}

### `materialize`

{{ @doc.cli.commands.materialize:usage }}

{{ @doc.cli.commands.materialize:description }}

### `--help`, `-h`

{{ @doc.cli.commands.help:usage }}, `standardoc -h`

{{ @doc.cli.commands.help:description }}

---

## `standardoc-server` — daemon

```
standardoc-server <transport> --workspace <path> [transport-specific args]
```

A single binary, four mutually exclusive transports — pick exactly one.

### `--mcp`

{{ @doc.cli.transports.mcp:usage }}

{{ @doc.cli.transports.mcp:description }}

### `--lsp`

{{ @doc.cli.transports.lsp:usage }}

{{ @doc.cli.transports.lsp:description }}

### `--web --port <N>`

{{ @doc.cli.transports.web:usage }}

{{ @doc.cli.transports.web:description }}

### `--export --out <dir>`

{{ @doc.cli.transports.export:usage }}

{{ @doc.cli.transports.export:description }}

### `--workspace <path>` *(required)*

Absolute or relative path to the workspace root. Standardoc treats this as the indexing scope and also looks here for `.standardoc.json` and `.standardoc/languages/*.json`.

### Exit codes

- `0` — clean shutdown
- `1` — runtime error (bind failure, scan error, etc.)
- `2` — argument error

---

## Environment variables

Standardoc itself reads only one environment variable directly :

- `RUST_LOG` — controls log level for `tracing` (e.g. `RUST_LOG=standardoc=debug`)

Install scripts (`scripts/install.{sh,ps1}`) honor :

- `STANDARDOC_VERSION` — pin to a specific release (e.g. `v0.1.0`)
- `STANDARDOC_HOME` — install root (default `$HOME/.standardoc` or `$env:USERPROFILE\.standardoc`)
- `STANDARDOC_NO_PATH` — skip the PATH suggestion message

## Configuration file

`.standardoc.json` at the workspace root — fully optional. See the
[Configuration section in README.md](../README.md#configuration-standardocjson)
for the full schema.

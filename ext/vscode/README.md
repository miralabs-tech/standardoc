# Standardoc — VSCode extension

AI-readable code documentation indexer for Rust and TypeScript projects, surfaced
over LSP (writer daemon) and MCP (read-only daemon) via stdio.

> **Status — scaffold mode.** This package contains types, wiring, and command
> registrations only. Daemon resolution, LSP/MCP client startup, and command
> bodies all throw at runtime until Phase B lands.

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

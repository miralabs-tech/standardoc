# standardoc-graph-viz — playground

Standalone dev harness for the WASM graph engine. Talks to a **live**
standardoc daemon (the one your VSCode extension is already running)
via the MCP HTTP transport. No extension rebuild required for engine
iteration.

## Quick start

```bash
# 1. Build the wasm crate
bun run build:wasm

# 2. Install dev deps (first time only)
bun install

# 3. Start the dev server
bun run dev
# → http://localhost:3000
```

The server reads `<repo>/.standardoc/mcp.endpoint` to discover the
daemon URL. Run any standardoc daemon first — easiest is to keep your
VSCode window open on the repo; the extension already runs one.

## Workspace selection

Defaults to the standardoc repo root (`../../../` from this folder).
For another workspace:

```bash
STANDARDOC_WORKSPACE=/abs/path/to/other-workspace bun run dev
```

## Iterating on the Rust engine

### One-shot (recommended)

```pwsh
# From the playground folder
bun run dev:full
#   ↑ runs ./dev.ps1 which:
#     - checks bun / wasm-pack / cargo / cargo-watch (auto-installs the last one if missing)
#     - does an initial `wasm-pack build` so /pkg/ is populated
#     - launches cargo-watch (rebuilds wasm on Rust source change)
#     - launches `bun --hot server.ts` (hot-reloads the playground)
#     - merges both stdouts into the current shell with [wasm] / [playground] prefixes
#     - Ctrl+C cleanly stops both jobs
```

Then reload the browser tab after each wasm rebuild (the wasm-pack
finishes typically in under a second on incremental builds).

### Manual (two terminals)

```bash
# Terminal 1 — auto-rebuild the wasm on file change.
cargo watch -w crates/standardoc-graph-viz/src \
  -s "wasm-pack build crates/standardoc-graph-viz --target web --out-dir pkg --dev"

# Terminal 2 — Bun's --hot already refreshes the playground.
cd crates/standardoc-graph-viz/playground && bun run dev
```

The wasm file is served with `Cache-Control: no-store`, so a plain
browser refresh always loads the latest build.

## Architecture

```
┌──────────────┐   /mcp/*    ┌──────────────────────┐
│   browser    │ ──────────▶ │ Bun dev server       │
│  (this app)  │             │ - serves index.html  │
│              │             │ - serves pkg/*.wasm  │
│  ┌────────┐  │             │ - proxies /mcp → ... │
│  │ engine │  │             └──────────┬───────────┘
│  │ (wasm) │  │                        │
│  └────────┘  │                        ▼
└──────────────┘             ┌──────────────────────┐
                             │ standardoc daemon    │
                             │ 127.0.0.1:<port>     │
                             │ (read from           │
                             │  .standardoc/        │
                             │  mcp.endpoint)       │
                             └──────────────────────┘
```

The MCP orchestration in `main.ts` mirrors `panel.ts:runBrowse` in the
extension — same `list_symbols` fan-out, same JSON payload shape.
When the engine API stabilises, the playground keeps existing while
the extension swaps its Preact `BrowseCanvas` for the same WASM
engine without any data-layer change.

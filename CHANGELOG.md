# Changelog

Per-release notes live alongside the published release tags — see the release
page for any tagged version to find what shipped in that version. This file
is intentionally minimal and only buffers in-flight work between releases.

## [Unreleased]

Targeting `v1.0.0-beta.1`. Major rewrite from the v0 prototype:

- AST-direct Rust + TS providers (`syn`, `swc`)
- SQLite + FTS5 graph storage (zero-duplication external content)
- 2 MCP tools day-1 (`find_symbol`, `get_context`)
- Single binary `stdoc` with sub-commands (LSP, MCP, index, rescan, watch, query, purge-excluded)
- LSP daemon = primary writer, MCP daemons = read-only
- VSCode extension with daemon supervisor + init opt-in flow
- AI agent skill generation for Claude Code
- MDX/React rendering layer (npm package, replaces the v0 DSL templating) — held back post-beta.2
- Virtual annotations enrichments, cross-language bridge plug-ins — held back post-beta.1

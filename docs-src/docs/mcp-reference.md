# MCP reference

📖 English · [Français](mcp-reference.fr.md)

Standardoc speaks the [Model Context Protocol](https://modelcontextprotocol.io/) over stdio. This document lists every tool and resource the `standardoc-server --mcp` daemon exposes, derived live from the source annotations — so it never drifts away from what the binary actually serves.

**Setup** — drop a `.mcp.json` at your workspace root :

```json
{
  "mcpServers": {
    "standardoc": {
      "type": "stdio",
      "command": "/absolute/path/to/standardoc-server",
      "args": ["--mcp", "--workspace", "/absolute/path/to/your/project"]
    }
  }
}
```

Then launch any MCP-aware client (Claude Code, Cursor, Zed, Continue, …). The daemon scans once at boot, watches the workspace for changes, and pushes notifications to your client when the index changes.

---

## Read & navigation

Tools that read the index, search by query, evaluate DSL, and extract raw comments.
{{ each t in @docs.module(mcp.tools) }}{{ if t.category == "navigation" }}
{{ if t.label == "args" }}

**Input schema** :

```dsl
{{ t.schema }}
```
{{ else }}

### ``{{ t.label }}``

{{ t.description }}
{{ /if }}{{ /if }}{{ /each }}

---

## Cross-reference

Type-graph navigation — find usages, implementations, and parameter/return-type matches by short name.
{{ each t in @docs.module(mcp.tools) }}{{ if t.category == "cross-ref" }}
{{ if t.label == "args" }}

**Input schema** :

```dsl
{{ t.schema }}
```
{{ else }}

### ``{{ t.label }}``

{{ t.description }}
{{ /if }}{{ /if }}{{ /each }}

---

## LSP semantic exposure

These mirror behaviors the LSP gives editors, exposed as MCP tools so agents (which don't speak LSP) can use them.
{{ each t in @docs.module(mcp.tools) }}{{ if t.category == "lsp" }}
{{ if t.label == "args" }}

**Input schema** :

```dsl
{{ t.schema }}
```
{{ else }}

### ``{{ t.label }}``

{{ t.description }}
{{ /if }}{{ /if }}{{ /each }}

---

## Quality & validation

Validator output, coverage stats, and tools to surface or audit documentation gaps.
{{ each t in @docs.module(mcp.tools) }}{{ if t.category == "quality" }}
{{ if t.label == "args" }}

**Input schema** :

```dsl
{{ t.schema }}
```
{{ else }}

### ``{{ t.label }}``

{{ t.description }}
{{ /if }}{{ /if }}{{ /each }}

---

## Agent docs emission

Generate well-known agent-oriented documentation formats (`llms.txt`, `skill.md`, OpenAPI, …) from your live workspace scan.
{{ each t in @docs.module(mcp.tools) }}{{ if t.category == "emit" }}
{{ if t.label == "args" }}

**Input schema** :

```dsl
{{ t.schema }}
```
{{ else }}

### ``{{ t.label }}``

{{ t.description }}
{{ /if }}{{ /if }}{{ /each }}

---

## Lifecycle & runtime

Force rescans, fetch the DSL reference, control the watcher.
{{ each t in @docs.module(mcp.tools) }}{{ if t.category == "lifecycle" }}
{{ if t.label == "args" }}

**Input schema** :

```dsl
{{ t.schema }}
```
{{ else }}

### ``{{ t.label }}``

{{ t.description }}
{{ /if }}{{ /if }}{{ /each }}

---

## Resources

MCP resources are read-only data exposed under URIs.
{{ each r in @docs.module(mcp.resources) }}
### ``{{ r.uri }}``

{{ r.description }}

**MIME type** : ``{{ r.mime }}``
{{ /each }}

Subscribe to a resource (per MCP spec) to receive updates when the underlying data changes.

---

## Notifications

The daemon pushes JSON-RPC notifications when state changes. Hosts that subscribe see :

- `notifications/standardoc/index_changed` — `{ revision, added, removed }` (DocKey lists, no full blocks — refetch via `get_doc` if needed) after every successful rescan
- `notifications/standardoc/diagnostics` — `{ path, diagnostics: [...] }` when validator output changes for a file
- `notifications/standardoc/config_reloaded` — `{ config }` after `.standardoc.json` is edited

These let agents react to file changes without polling — same delivery mechanism the LSP uses for `publishDiagnostics`.

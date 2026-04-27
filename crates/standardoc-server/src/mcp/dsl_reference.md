# Standardoc DSL reference (v2)

This reference covers both halves of the system:

- **Source-side annotations** — `@doc`, `@doc-extend`, `@param`, etc. — that
  write data into the index when scanning source files.
- **Template-side DSL** — `{{ @doc.KEY:tag }}`, `{{ each x in @docs.… }}`,
  etc. — that read that data back when rendering markdown templates.

Use this inside markdown templates. Expressions are wrapped in `{{ … }}`.
The `@` prefix on `@doc.KEY` and `@docs.…` signals a Standardoc reference —
other `{{ … }}` content (Astro/MDX expressions, etc.) is passed through.

## Mental model

There are **two namespaces** on the template side:

- `@doc.KEY` — refers to **one specific block** by its key. Use this for
  documentation written *about* a known function/class/module.
- `@docs.…` — refers to a **set of blocks** for iteration. Use this when
  you want to generate per-module pages without listing each function by hand.

After a block reference, `:access` projects something out of it. After an
alias inside `each`, `.path` does the same.

**Key paths use `.` for FQN segments and `::` for satellite annotations**
(see `@doc-extend` below). So `@doc.tools.get_doc::schema` resolves the
`schema` satellite of the anchor `tools.get_doc`.

## Source-side annotation syntax

Annotations live inside source-code comments and feed the index. The scanner
is **comment-style agnostic** — every supported language exposes the same
markers to the same parser:

| Language    | Single-line | Doc single-line | Block        | Doc block    |
| ----------- | ----------- | --------------- | ------------ | ------------ |
| Rust        | `//`        | `///`, `//!`    | `/* … */`    | `/** … */`   |
| TypeScript  | `//`        | —               | `/* … */`    | `/** … */`   |
| Python      | `#`         | —               | (docstring)  | —            |
| Lua         | `--`        | `---`           | `--[[ … ]]`  | —            |

Any marker variant works: an annotation written `// @doc foo.bar` in a plain
single-line comment is recognized identically to `/// @doc foo.bar` written
as Rustdoc — the difference is only whether the symbol attaches.

### Anchors — `@doc K [LABEL]`

Declares a documentable block at the canonical key `K`. Two attachment modes:

**Symbol-attached** — comment sits directly above a parsed symbol
(function, struct, class, etc.):

```rust
/// @doc validator.rules.std001 Duplicate DocKey
/// @description Two annotations resolve to the same FQN.
fn rule_std001_duplicate_key(...) { ... }
```

The block is **hybrid**: the explicit `@doc` overrides the auto-inferred key,
and the symbol's `kind`/`signature`/`params` are still attached.

**Free-floating** — comment in any plain block, with no symbol below:

```rust
// @doc mcp.tools.search_docs search_docs
// @description Search the doc index by query.
// @category navigation
```

The block is **annotated**, `symbol` is `None`. Use this when the doc key has
no single source-of-truth symbol — shared handlers, virtual concepts,
external API surfaces.

`LABEL` is optional — when omitted, falls back to the last segment of the
key. `LABEL` may contain spaces (everything after the key is taken as label).

### Satellites — `@doc-extend ANCHOR EXTENDED`

One-line, two whitespace-separated args. Produces a child block at key
`ANCHOR::EXTENDED`. Other tags inside the same comment block attach to the
satellite, **not** to the anchor:

```rust
// @doc-extend mcp.tools.get_doc args
// @schema {"type":"object","properties":{"key":{"type":"string"}}}
```

Result: a block at `mcp.tools.get_doc::args` with the `schema` tag set.
The `doc-extend` tag itself is stripped from the satellite's tag map.

Motivating use case: heavy annotations (JSON schemas, long examples,
narrative blurbs) for an entry in a dense registry. Keep the anchor on the
production symbol, push the noisy payload to a dedicated `*_satellites.rs`
file. The pipeline unions everything under the anchor key when iterating
via `@docs.module(ANCHOR)`.

### Tags — `@TAG args...`

A tag occupies one line (single-line tags) or extends across multiple lines
until the next `@tag` or end of the comment block (multi-line tags — see
below). Tag names accept `[a-zA-Z0-9_.-]`, leading char must be alphanumeric
or `_`. Stray dashes (`@-foo`) are not parsed as tags.

**Built-in cardinalities and field shapes**:

| Tag                   | Fields                       | Cardinality |
| --------------------- | ---------------------------- | ----------- |
| `param`               | `[name, type, description]`  | Multi       |
| `returns` / `return`  | `[type, description]`        | Single      |
| `description`         | `[content]`                  | Single      |
| `example`             | `[content]`                  | Multi       |
| `since`               | `[version]`                  | Single      |
| `deprecated`          | `[reason]`                   | Single      |
| `see`                 | `[target]`                   | Multi       |

**Custom tags** are declared under `tags:` in `.standardoc.json`:

```json
{
  "tags": {
    "category": { "fields": ["value"], "cardinality": "single" },
    "exit-code": { "fields": ["code", "description"], "cardinality": "multi" }
  }
}
```

`cardinality` defaults to `"single"`. `fields` controls how the value
splits — `parse_field`-aware (whitespace-split, fields named for projection
via `:tag.field`).

Undeclared custom tags still parse (whitespace-split fields), but only
`@param`/`@returns`/etc. have field-aware schemas wired up.

### Multi-line tag bodies

Three tags accept content that spans multiple lines:

```
description    example    schema
```

The body extends from the tag declaration line down to the next `@tag` or
the end of the comment block. The whole markdown is preserved verbatim
(including bullets, fenced blocks, and `@`-prefixed prose that isn't a
recognized tag at the start of a line):

```rust
/// @doc cli.commands.scan scan
/// @category index
/// @description
/// Walk `<path>` and emit canonical `DocBlock` entries as JSON, one block
/// per record.
///
/// **Exit codes**:
/// - `0` — success
/// - `1` — pipeline error
/// - `2` — missing required argument
fn run_scan(...) { ... }
```

Projecting `:description` returns the full block. Note that `@description`
must come **after** any single-line tags that would otherwise consume the
prose — once a multi-line tag opens, every following line up to the next
`@tag` is its body.

### Implicit description

Prose **before** the first `@tag` inside a doc comment becomes an implicit
`@description`:

```rust
/// Says hello to the named user.
/// @doc greetings.hello
fn hello(name: &str) { ... }
```

Here `description` = "Says hello to the named user." even though no
`@description` is written explicitly. An explicit `@description` always
wins over implicit prose.

## Default projection — `{{ @doc.KEY }}` (no `:`)

Bare references produce a useful summary by default:

- symbol + description → `{signature}\n\n{description}`
- symbol only → `{signature}`
- description only → `{description}`
- neither → `label`

So `### {{ @doc.foo }}` "just works" without thinking about the access path.

## Single-block accessors — `@doc.KEY:…`

### Block fields
- `:label` — block label (last segment of FQN)
- `:key` — full block key
- `:origin` — `"inferred"` | `"annotated"` | `"hybrid"`

### Source location (whitelist)
- `:meta.path` — file path (forward slashes, even on Windows)
- `:meta.lineStart` / `:meta.lineEnd` — 1-indexed lines
- `:meta.column` — 1-indexed column
- `:meta.fileExt` — extension without dot
- `:meta.commentStyle` — `"single-line"` | `"multi-line"` | `"doc-single"` | `"doc-multi"`

### Symbol info (whitelist, only if symbol is present)
- `:symbol.signature` — canonical one-line signature
- `:symbol.kind` — `"function"` | `"method"` | `"class"` | `"struct"` | …
- `:symbol.visibility` — `"public"` | `"private"` | `"crate"` | `"internal"` | `"inherited"`
- `:symbol.isAsync` — `"true"` | `"false"`
- `:symbol.isDeprecated` — `"true"` | `"false"`
- `:symbol.generics` — comma-joined
- `:symbol.decorators` — comma-joined

> Internal fields (params, returns sub-objects, mtimes, references) are **not**
> exposed by design — use the `:param` / `:returns` tags instead.

### Tag access

Each tag has a **cardinality**: `Single` (one occurrence allowed) or `Multi`.

#### Single tags
- `:TAG` — joined fields of the (single) occurrence
- `:TAG.FIELD` — named field of the occurrence (via schema)

```md
{{ @doc.foo:description }}
{{ @doc.foo:returns.type }}
```

#### Multi tags
Standalone `:TAG` and `:TAG.FIELD` are **errors** on Multi (ambiguous).
Pick one:

- `:TAG[N]` — Nth occurrence, fields joined
- `:TAG[N].FIELD` — named field of the Nth occurrence
- `:first(TAG)` / `:last(TAG)` — sugar for `[0]` / `[-1]`
- `:first(TAG).FIELD` / `:last(TAG).FIELD` — chained
- `each x in @doc.foo:TAG` — iterate

```md
{{ @doc.foo:param[0].name }}
{{ @doc.foo:first(param).name }}
{{ @doc.foo:last(see).target }}
```

### Functions
- `:has(TAG)` — `"true"` | `"false"`
- `:count(TAG)` — integer as string
- `:first(TAG)` / `:last(TAG)` — see above (chainable with `.FIELD`)

`has` and `count` return scalars — chaining `.FIELD` after them is an error.

## Multi-block iteration — `@docs.…`

```md
{{ each f in @docs.module(api.users) }}
### {{ f.label }}
{{ f }}
{{ /each }}
```

Sources:
- `@docs.module(KEY)` — the anchor at `KEY` plus every block whose key starts
  with `KEY.` (dot-children) or `KEY::` (satellites). Strict segment
  boundary — `module(api.user)` does NOT match `api.users.*`
- `@docs.satellites(KEY)` — only the satellites under `KEY` (`KEY::*`),
  excluding the anchor itself and any dot-children
- `@docs.all` — every block in the index

The alias (`f` above) is bound to a **block**, so `f.PATH` follows the same
rules as `:PATH` after `@doc.KEY`:
- `f` (bare) — default projection
- `f.label` / `f.key` / `f.origin` / `f.meta.path` / `f.symbol.signature` — fields
- `f.description` — Single tag → joined fields
- `f.returns.type` — Single tag shortcut
- `f.param` — **error** (Multi tag, use nested each or first/last)

## Single-tag iteration — `each x in @doc.K:TAG`

```md
{{ each p in @doc.foo:param }}
- **{{ p.name }}** (`{{ p.type }}`): {{ p.description }}
{{ /each }}
```

Inside the body, `p` is bound to a tag occurrence. `{{ p }}` (bare) joins
all fields with space. `{{ p.FIELD }}` resolves via the tag schema.

## Conditionals

```md
{{ if @doc.foo:has(example) }}
Example: {{ @doc.foo:first(example) }}
{{ else if @doc.foo:has(deprecated) }}
**Deprecated**: {{ @doc.foo:deprecated.reason }}
{{ else }}
*No example.*
{{ /if }}
```

Truthy rules for `if @doc.K:X`:
- `X` rendering produces non-empty string → true
- `X` is a missing tag / field → **false** (no need for `has()` everywhere)
- `X` is ambiguous (Multi tag bare access) → **error** (template bug)

Comparisons:
- `if @doc.K:count(TAG) > 0`
- `if @doc.K:symbol.visibility == "public"`
- Operators: `==` `!=` `>` `<` `>=` `<=`

## Closing tags

Closing tags are **typed**:
- `{{ /each }}` closes an `each`
- `{{ /if }}` closes an `if` (with or without `else`/`else if`)

Mismatched (`{{ /if }}` against an open `each`) gives a clear error.
The legacy generic `{{ end }}` is rejected.

## Whitespace

Block directives alone on a line (`each`, `/each`, `if`, `else`, `else if`,
`/if`) consume the entire line — no phantom blank lines in the rendered
output. Inline directives are preserved as-is.

## Code blocks and inline code

CommonMark fences and inline backticks decide whether `{{ … }}` is
evaluated or passed through verbatim. This is what lets a doc page quote
DSL syntax without the renderer trying to resolve the example.

### Fenced blocks (` ``` `, `~~~`)

By default, every `{{ … }}` inside a fenced block is **passthrough**:

````md
```
{{ @doc.foo }}    ← left as literal text
```
````

To opt into evaluation, use the info-string `dsl` (case-insensitive):

````md
```dsl
{{ @doc.foo:description }}    ← evaluated against the index
```
````

The marker line itself is always literal — DSL never runs on the opening or
closing fence line. Other info-strings (`rust`, `json`, `sh`, `md`, …) keep
the passthrough behavior.

### Inline backticks

A single backtick toggles "in code". The parser flips state on every `` ` ``
it sees, so:

- `` `{{ X }}` `` — 1 backtick opens code → passthrough → 1 backtick closes
- `` ``{{ X }}`` `` — 2 backticks toggle twice → back to "outside code" →
  the DSL evaluates → 2 backticks toggle twice again → output is a single
  inline-code span containing the rendered value

Use the double-backtick form to inject a DSL expression *as inline code* in
the rendered output. The current parser does not implement a separate
"double-backtick span" CommonMark rule; the toggle is what produces the
effect.

## Quick reference — common patterns

```md
{{ @doc.foo }}                              # smart default
{{ @doc.foo:description }}                  # single tag
{{ @doc.foo:returns.type }}                 # single tag field
{{ @doc.foo:first(param).name }}            # multi tag, chained
{{ @doc.foo:meta.path }}:{{ @doc.foo:meta.lineStart }}

{{ each p in @doc.foo:param }}
- {{ p.name }} ({{ p.type }})
{{ /each }}

{{ each f in @docs.module(api.users) }}
## {{ f.label }}
{{ f }}
{{ /each }}

{{ @doc.tools.get_doc::schema:args-schema }}    # satellite projection

{{ each s in @docs.satellites(tools.get_doc) }}
### {{ s.label }}
{{ s }}
{{ /each }}

{{ if @doc.foo:has(example) }}
{{ @doc.foo:first(example) }}
{{ /if }}

``{{ @doc.foo:label }}``                    # inline code, evaluated
```

````md
```dsl
{{ @doc.foo:description }}                  # block code, evaluated
```
````

Source-side examples:

```rust
/// @doc api.users.create
/// @param body UserCreateInput Validated payload.
/// @returns User Newly persisted user.
/// @description Persist a new user from a validated payload.
fn create(body: UserCreateInput) -> User { ... }
```

```rust
// @doc mcp.tools.search_docs search_docs
// @category navigation
// @description Search the doc index by query.

// @doc-extend mcp.tools.search_docs args
// @schema {"type":"object","properties":{"query":{"type":"string"}}}
```

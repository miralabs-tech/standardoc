# Comparison — verbose vs smart

📖 English · [Français](COMPARISON.fr.md)

You have two templates in `docs-src/`. Both produce **exactly the same final render** (look at `docs-rendered/`). The difference is what YOU write and maintain.

## The verdict in numbers

| | Template lines | Effort to add a 6th function |
|---|---|---|
| `01-api-verbose.md` | **~170 lines** | Edit the template (copy-paste a block, rename the key 8 times, hope you don't miss any) |
| `02-api-smart.md` | **~57 lines** | Nothing. The function appears automatically on the next build. |

And that's with **5 functions**. With 50, the verbose version blows past 1500 lines. The smart one is still 57.

## The visual difference (the part that changes)

### Verbose — 1 function = 1 block to duplicate

```markdown
### Create user

{{ @doc.users.create:description }}

​```rust
{{ @doc.users.create:symbol.signature }}
​```

{{ each p in @doc.users.create:param }}
- **{{ p.name }}** (`{{ p.type }}`): {{ p.description }}
{{ /each }}

**Returns** (`{{ @doc.users.create:returns.type }}`): {{ @doc.users.create:returns.description }}

### Get user by id

{{ @doc.users.get:description }}

​```rust
{{ @doc.users.get:symbol.signature }}
​```

{{ each p in @doc.users.get:param }}
- **{{ p.name }}** (`{{ p.type }}`): {{ p.description }}
{{ /each }}

**Returns** (`{{ @doc.users.get:returns.type }}`): {{ @doc.users.get:returns.description }}

### List users
... (and so on for each of the 10 functions)
```

### Smart — 1 loop for the 5 server functions

```markdown
{{ each f in @docs.module(users) }}
### {{ f.label }}

{{ f.description }}

​```rust
{{ f.symbol.signature }}
​```

{{ each p in f:param }}
- **{{ p.name }}** (`{{ p.type }}`): {{ p.description }}
{{ /each }}

**Returns** (`{{ f.returns.type }}`): {{ f.returns.description }}
{{ /each }}
```

That's it. For the 5 server functions. **And for the 50 the day you have 50.**

## What happens when you change the code

Imagine you decide to rename the `password` param to `pwd` in `server.rs` :

```diff
- /// @param password string clear-text password, hashed internally
+ /// @param pwd string clear-text password, hashed internally
- pub fn create(email: &str, password: &str) -> User { ... }
+ pub fn create(email: &str, pwd: &str) -> User { ... }
```

→ On the next build, **both templates** (verbose AND smart) produce a page where `password` has become `pwd` everywhere. **Zero `.md` file to edit.**

Compare with a manual approach (Docusaurus, Nextra, FiveM-style) : you'd have a `.md` page that still says "password" until a human thinks to update it. That's exactly the scenario where the doc drifts from the code and where, 6 months later, people stop trusting the doc.

## What about template aesthetics ?

Yes, written like this in raw Markdown without highlighting, the template looks like a mix of `.md` + `mustache`. But :

1. **You write it once**, then almost never come back to it.
2. **The VSCode extension** (LSP in place core-side, wrapper extension in progress) :
   - colors the DSL differently from the markdown
   - autocomplete on `@doc.` → list of your blocks
   - hover on a `{{ @doc.users.create:returns.type }}` → shows you `User` live
   - goto-definition → sends you to the corresponding Rust function
   - rename refactoring → renames in code AND propagates in all `.md` files
3. **The final render** (`docs-rendered/`) is clean markdown, read by GitHub / VSCode preview / a website — exactly what your end user sees.

You don't write the render, you write the "recipe". The recipe is less pretty than the dish, that's normal — but you write it ONCE and you eat 50 times.

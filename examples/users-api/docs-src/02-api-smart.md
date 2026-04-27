# Users API

API pour gérer les utilisateurs. Côté serveur (Rust) + côté client (TypeScript SDK).

> ✨ **Version "smart"** de ce template. Tu vois en bas — pas de `users.create`,
> `users.get`, `users.list` énumérés à la main. C'est `@docs.module(...)` qui
> itère sur tout ce qui matche. **Ajoute une 6ème fonction dans `server.rs`,
> elle apparaît automatiquement.**

## Server (Rust)

{{ each f in @docs.module(users) }}
### {{ f.label }}

{{ f.description }}

```rust
{{ f.symbol.signature }}
```

{{ each p in f:param }}
- **{{ p.name }}** (`{{ p.type }}`): {{ p.description }}
{{ /each }}

**Returns** (`{{ f.returns.type }}`): {{ f.returns.description }}

{{ if f:has(deprecated) }}
> ⚠️ **Deprecated**: {{ f.deprecated.reason }}
{{ /if }}

_Since {{ f.since.version }}_

{{ /each }}

## Client SDK (TypeScript)

{{ each f in @docs.module(client.users) }}
### {{ f.label }}

{{ f.description }}

```ts
{{ f.symbol.signature }}
```

{{ each p in f:param }}
- **{{ p.name }}** (`{{ p.type }}`): {{ p.description }}
{{ /each }}

**Returns** (`{{ f.returns.type }}`): {{ f.returns.description }}

{{ if f:has(deprecated) }}
> ⚠️ **Deprecated**: {{ f.deprecated.reason }}
{{ /if }}

{{ /each }}

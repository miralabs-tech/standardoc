# Greet — end-to-end

Standardoc pulls documentation out of the Rust server **and** the TypeScript
client from the same scan. Both are rendered into one page below.

## Server — Rust

{{ @doc.server.greet:description }}

```rust
{{ @doc.server.greet:symbol.signature }}
```

**Parameters**

{{ each p in @doc.server.greet:param }}
- **{{ p.name }}** (`{{ p.type }}`): {{ p.description }}
{{ /each }}

**Returns** (`{{ @doc.server.greet:returns.type }}`): {{ @doc.server.greet:returns.description }}

Source: `{{ @doc.server.greet:meta.path }}`

## Client — TypeScript

{{ @doc.client.greet:description }}

```ts
{{ @doc.client.greet:symbol.signature }}
```

**Parameters**

{{ each p in @doc.client.greet:param }}
- **{{ p.name }}** (`{{ p.type }}`): {{ p.description }}
{{ /each }}

**Returns** (`{{ @doc.client.greet:returns.type }}`): {{ @doc.client.greet:returns.description }}

Source: `{{ @doc.client.greet:meta.path }}`

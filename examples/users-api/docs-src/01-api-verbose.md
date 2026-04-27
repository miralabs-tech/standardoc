# Users API

API pour gérer les utilisateurs. Côté serveur (Rust) + côté client (TypeScript SDK).

> ⚠️ **Version "verbeuse"** de ce template — chaque fonction est listée à la main.
> Ajouter une nouvelle fonction = éditer ce fichier. Voir `02-api-smart.md` pour
> la version qui se met à jour toute seule.

## Server (Rust)

### Create user

{{ @doc.users.create:description }}

```rust
{{ @doc.users.create:symbol.signature }}
```

{{ each p in @doc.users.create:param }}
- **{{ p.name }}** (`{{ p.type }}`): {{ p.description }}
{{ /each }}

**Returns** (`{{ @doc.users.create:returns.type }}`): {{ @doc.users.create:returns.description }}

_Since {{ @doc.users.create:since.version }}_

### Get user by id

{{ @doc.users.get:description }}

```rust
{{ @doc.users.get:symbol.signature }}
```

{{ each p in @doc.users.get:param }}
- **{{ p.name }}** (`{{ p.type }}`): {{ p.description }}
{{ /each }}

**Returns** (`{{ @doc.users.get:returns.type }}`): {{ @doc.users.get:returns.description }}

_Since {{ @doc.users.get:since.version }}_

### List users

{{ @doc.users.list:description }}

```rust
{{ @doc.users.list:symbol.signature }}
```

{{ each p in @doc.users.list:param }}
- **{{ p.name }}** (`{{ p.type }}`): {{ p.description }}
{{ /each }}

**Returns** (`{{ @doc.users.list:returns.type }}`): {{ @doc.users.list:returns.description }}

_Since {{ @doc.users.list:since.version }}_

### Update user

{{ @doc.users.update:description }}

```rust
{{ @doc.users.update:symbol.signature }}
```

{{ each p in @doc.users.update:param }}
- **{{ p.name }}** (`{{ p.type }}`): {{ p.description }}
{{ /each }}

**Returns** (`{{ @doc.users.update:returns.type }}`): {{ @doc.users.update:returns.description }}

_Since {{ @doc.users.update:since.version }}_

### Delete user

{{ @doc.users.delete:description }}

```rust
{{ @doc.users.delete:symbol.signature }}
```

{{ each p in @doc.users.delete:param }}
- **{{ p.name }}** (`{{ p.type }}`): {{ p.description }}
{{ /each }}

**Returns** (`{{ @doc.users.delete:returns.type }}`): {{ @doc.users.delete:returns.description }}

{{ if @doc.users.delete:has(deprecated) }}
> ⚠️ **Deprecated**: {{ @doc.users.delete:deprecated.reason }}
{{ /if }}

_Since {{ @doc.users.delete:since.version }}_

## Client SDK (TypeScript)

### Create user (client)

{{ @doc.client.users.create:description }}

```ts
{{ @doc.client.users.create:symbol.signature }}
```

{{ each p in @doc.client.users.create:param }}
- **{{ p.name }}** (`{{ p.type }}`): {{ p.description }}
{{ /each }}

**Returns** (`{{ @doc.client.users.create:returns.type }}`): {{ @doc.client.users.create:returns.description }}

### Get user (client)

{{ @doc.client.users.get:description }}

```ts
{{ @doc.client.users.get:symbol.signature }}
```

{{ each p in @doc.client.users.get:param }}
- **{{ p.name }}** (`{{ p.type }}`): {{ p.description }}
{{ /each }}

**Returns** (`{{ @doc.client.users.get:returns.type }}`): {{ @doc.client.users.get:returns.description }}

### List users (client)

{{ @doc.client.users.list:description }}

```ts
{{ @doc.client.users.list:symbol.signature }}
```

{{ each p in @doc.client.users.list:param }}
- **{{ p.name }}** (`{{ p.type }}`): {{ p.description }}
{{ /each }}

**Returns** (`{{ @doc.client.users.list:returns.type }}`): {{ @doc.client.users.list:returns.description }}

### Update user (client)

{{ @doc.client.users.update:description }}

```ts
{{ @doc.client.users.update:symbol.signature }}
```

{{ each p in @doc.client.users.update:param }}
- **{{ p.name }}** (`{{ p.type }}`): {{ p.description }}
{{ /each }}

**Returns** (`{{ @doc.client.users.update:returns.type }}`): {{ @doc.client.users.update:returns.description }}

### Delete user (client)

{{ @doc.client.users.delete:description }}

```ts
{{ @doc.client.users.delete:symbol.signature }}
```

{{ each p in @doc.client.users.delete:param }}
- **{{ p.name }}** (`{{ p.type }}`): {{ p.description }}
{{ /each }}

**Returns** (`{{ @doc.client.users.delete:returns.type }}`): {{ @doc.client.users.delete:returns.description }}

{{ if @doc.client.users.delete:has(deprecated) }}
> ⚠️ **Deprecated**: {{ @doc.client.users.delete:deprecated.reason }}
{{ /if }}

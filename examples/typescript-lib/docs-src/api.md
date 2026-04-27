# Users API

## `{{ @doc.api.types.User:label }}`

{{ @doc.api.types.User:description }}

```ts
{{ @doc.api.types.User:symbol.signature }}
```

---

## `{{ @doc.api.users.create:label }}`

{{ @doc.api.users.create:description }}

```ts
{{ @doc.api.users.create:symbol.signature }}
```

### Parameters

{{ each p in @doc.api.users.create:param }}
- **{{ p.name }}** (`{{ p.type }}`): {{ p.description }}
{{ /each }}

**Returns** (`{{ @doc.api.users.create:returns.type }}`): {{ @doc.api.users.create:returns.description }}

---

## `{{ @doc.api.users.delete:label }}`

{{ @doc.api.users.delete:description }}

```ts
{{ @doc.api.users.delete:symbol.signature }}
```

### Parameters

{{ each p in @doc.api.users.delete:param }}
- **{{ p.name }}** (`{{ p.type }}`): {{ p.description }}
{{ /each }}

**Returns** (`{{ @doc.api.users.delete:returns.type }}`): {{ @doc.api.users.delete:returns.description }}

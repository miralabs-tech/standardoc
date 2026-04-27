# {{ @doc.calculator:label }}

{{ @doc.calculator:description }}

## `{{ @doc.calculator.add:label }}`

{{ @doc.calculator.add:description }}

### Signature

```rust
{{ @doc.calculator.add:symbol.signature }}
```

### Parameters

{{ each p in @doc.calculator.add:param }}
- **{{ p.name }}** (`{{ p.type }}`): {{ p.description }}
{{ /each }}

**Returns** (`{{ @doc.calculator.add:returns.type }}`): {{ @doc.calculator.add:returns.description }}

{{ if @doc.calculator.add:has(example) }}
### Example

```rust
{{ @doc.calculator.add:first(example) }}
```
{{ /if }}

---

Source: [{{ @doc.calculator.add:meta.path }}:{{ @doc.calculator.add:meta.lineStart }}]({{ @doc.calculator.add:meta.path }})

# standarx-dsl

Reusable DSL parser for the `standar*` ecosystem (lexer + parser + AST).

The DSL grammar supports nested blocks, arrays, scalar literals, and
interpolated strings — designed to host config files like
`standarbuild`'s `.sxb` (task definitions) and `standardoc`'s `.sxd`
(workspace config).

## Status

Pre-1.0. The grammar is stable; the public Rust API may evolve. Pin to
exact `0.1.x` until v1.0.

## Usage

```rust
let src = r#"
    version "0.1.0"
    project "ext" {
        path "./ext"
        tasks ["build" "test"]
    }
"#;
let file = standarx_dsl::parse(src).expect("parse ok");
assert_eq!(file.stmts.len(), 2);
```

The parser is consumer-agnostic: it produces a generic `ast::File` tree.
Downstream crates lower it into their own typed schemas (no schema
opinions baked in here).

## Origin

Extracted from
[standarbuild](https://github.com/miralabs-tech/standarbuild)'s
internal `dsl/` module so multiple `standar*` projects can share a
single parser.

## License

MIT

//! Built-in `TagSchema`s — what fields each well-known tag has, in order.
//!
//! Users can override any of these via `tags: { ... }` in `.standardoc.json`.
//! Anything not listed here falls back to positional access (`[0]`, `[1]`, ...)
//! until a schema is provided.

use crate::config::{TagCardinality, TagSchema};
use std::collections::BTreeMap;

pub fn builtin_schemas() -> BTreeMap<String, TagSchema> {
    let mut m = BTreeMap::new();
    // Multi: as many `@param` as arguments -> user must choose
    // `[n]`, `first()`, `last()` or `each` to disambiguate.
    m.insert(
        "param".to_owned(),
        TagSchema {
            fields: vec![
                "name".to_owned(),
                "type".to_owned(),
                "description".to_owned(),
            ],
            required: vec!["name".to_owned()],
            cardinality: TagCardinality::Multi,
        },
    );
    // Single : un seul `@returns` par fonction. `:returns.type` direct.
    let returns = TagSchema {
        fields: vec!["type".to_owned(), "description".to_owned()],
        required: vec!["type".to_owned()],
        cardinality: TagCardinality::Single,
    };
    m.insert("returns".to_owned(), returns.clone());
    m.insert("return".to_owned(), returns);
    // Multi: we can have multiple examples.
    m.insert(
        "example".to_owned(),
        TagSchema {
            fields: vec!["content".to_owned()],
            required: vec![],
            cardinality: TagCardinality::Multi,
        },
    );
    // Single : une description par bloc.
    m.insert(
        "description".to_owned(),
        TagSchema {
            fields: vec!["content".to_owned()],
            required: vec![],
            cardinality: TagCardinality::Single,
        },
    );
    m.insert(
        "since".to_owned(),
        TagSchema {
            fields: vec!["version".to_owned()],
            required: vec!["version".to_owned()],
            cardinality: TagCardinality::Single,
        },
    );
    m.insert(
        "deprecated".to_owned(),
        TagSchema {
            fields: vec!["reason".to_owned()],
            required: vec![],
            cardinality: TagCardinality::Single,
        },
    );
    // Multi : multiple `@see` references possibles.
    m.insert(
        "see".to_owned(),
        TagSchema {
            fields: vec!["target".to_owned()],
            required: vec!["target".to_owned()],
            cardinality: TagCardinality::Multi,
        },
    );
    m
}

/// Merges a user-provided `tags` config over the built-in schemas.
/// User entries win; built-ins provide the fallback for anything not overridden.
pub fn merged_schemas(user: &BTreeMap<String, TagSchema>) -> BTreeMap<String, TagSchema> {
    let mut merged = builtin_schemas();
    for (name, schema) in user {
        merged.insert(name.clone(), schema.clone());
    }
    merged
}

/// Resolves a named field of a tag occurrence.
///
/// Returns `Some(&field_value)` when the tag has a schema and the field name
/// maps to a populated position. `None` when either the schema is missing the
/// field or the occurrence doesn't have that many fields.
pub fn field_value<'a>(
    schemas: &BTreeMap<String, TagSchema>,
    tag: &str,
    field: &str,
    occurrence: &'a [String],
) -> Option<&'a String> {
    let schema = schemas.get(tag)?;
    let pos = schema.fields.iter().position(|f| f == field)?;
    occurrence.get(pos)
}

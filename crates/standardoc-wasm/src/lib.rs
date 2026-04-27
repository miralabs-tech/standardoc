//! WebAssembly bindings for the Standardoc core.
//!
//! Intentionally reduced surface: only expose layers that do not require
//! filesystem access or scanning. Target use cases are the **web playground**,
//! the **web `VSCode` extension**, and any browser-side client that already
//! has `DocBlock`s and only needs transforms (DSL render, validate,
//! llms/skill emit).
//!
//! ## API
//!
//! All exposed functions take and return **strings** — JSON-serialized for
//! complex structures. Intentional choice:
//!
//! - Trivial JS/TS interop (no complex `wasm-bindgen` typing to maintain)
//! - Stable schema even if WASM/JS ABI changes
//! - Negligible overhead compared to actual work (DSL eval, emit)
//!
//! Errors are surfaced as JS-side `Err(string)` via `JsError`.
//!
//! ## Architecture
//!
//! Each public function has two versions:
//!
//! - A `core_*` function returning `Result<String, String>` — pure Rust,
//!   testable on native target (`cargo test`).
//! - Un wrapper `#[wasm_bindgen]` qui convertit `String` → `JsError`. Ces
//!   wrappers only run on wasm target (`JsError::new` panics natively), so
//!   they are not covered by `cargo test`.
//!
//! ## What is NOT exposed
//!
//! - **`Watcher`** — pas de filesystem en wasm32-unknown-unknown.
//! - **`Scanner` / `Registry`** — depends on language providers (syn,
//!   tree-sitter, swc, rustpython) that are heavy to wasm-ize and not useful
//!   in the browser-side target flow (index is computed server-side and sent
//!   to the client).
//! - **`extract_block`** — same reason: depends on `LanguageProvider`.

#![forbid(unsafe_code)]

use standardoc_core::config::{Config, TagSchema};
use standardoc_core::dsl::{merged_schemas, render_string};
use standardoc_core::emit::{emit_llms_full, emit_llms_txt, emit_skill_md, EmitOptions};
use standardoc_core::model::DocBlock;
use standardoc_core::pipeline::KeyCollision;
use standardoc_core::validator::validate;
use std::collections::BTreeMap;
use wasm_bindgen::prelude::*;

// =====================================================================
// Pure layer (native-testable) returning Result<String, String>.
// =====================================================================

/// Version du crate.
#[must_use]
pub fn core_version() -> String {
    env!("CARGO_PKG_VERSION").to_owned()
}

/// Evaluate a DSL template against a block index.
///
/// `blocks_json` : JSON `Object<key, DocBlock>`.
/// `template_src`: template string with `{{ @doc.KEY:access }}`.
/// `schemas_json`: JSON `Object<tag_name, TagSchema>` (custom tags).
/// Pass `"{}"` to use only built-ins.
pub fn core_evaluate_dsl(
    blocks_json: &str,
    template_src: &str,
    schemas_json: &str,
) -> Result<String, String> {
    let blocks = parse_blocks(blocks_json)?;
    let user_schemas: BTreeMap<String, TagSchema> =
        serde_json::from_str(schemas_json).map_err(|e| format!("invalid schemas JSON: {e}"))?;
    render_string(template_src, &blocks, &user_schemas).map_err(|e| format!("render error: {e}"))
}

/// Run validator against an index. `collisions_json` can be `"[]"`.
/// `config_json` can be `"{}"` for defaults. Returns JSON
/// `Vec<Diagnostic>`.
pub fn core_validate_blocks(
    blocks_json: &str,
    collisions_json: &str,
    config_json: &str,
) -> Result<String, String> {
    let blocks = parse_blocks(blocks_json)?;
    let collisions: Vec<KeyCollision> = serde_json::from_str(collisions_json)
        .map_err(|e| format!("invalid collisions JSON: {e}"))?;
    let config: Config =
        serde_json::from_str(config_json).map_err(|e| format!("invalid config JSON: {e}"))?;
    // STD004/STD007 don't apply on the WASM side: we have no access to
    // the workspace's narrative pages (no FS). Pass an empty set to
    // disable those rules while keeping the API stable.
    let pages = BTreeMap::new();
    let diagnostics = validate(&blocks, &collisions, &pages, &config);
    serde_json::to_string(&diagnostics).map_err(|e| format!("serialize diagnostics: {e}"))
}

pub fn core_emit_llms_txt(blocks_json: &str, opts_json: &str) -> Result<String, String> {
    let blocks = parse_blocks(blocks_json)?;
    let opts = parse_opts(opts_json)?;
    Ok(emit_llms_txt(&blocks, &opts))
}

pub fn core_emit_llms_full(blocks_json: &str, opts_json: &str) -> Result<String, String> {
    let blocks = parse_blocks(blocks_json)?;
    let opts = parse_opts(opts_json)?;
    Ok(emit_llms_full(&blocks, &opts))
}

pub fn core_emit_skill_md(blocks_json: &str, opts_json: &str) -> Result<String, String> {
    let blocks = parse_blocks(blocks_json)?;
    let opts = parse_opts(opts_json)?;
    Ok(emit_skill_md(&blocks, &opts))
}

/// Return all effective tag schemas (built-ins + user) as JSON.
pub fn core_list_schemas(user_schemas_json: &str) -> Result<String, String> {
    let user: BTreeMap<String, TagSchema> = serde_json::from_str(user_schemas_json)
        .map_err(|e| format!("invalid user schemas JSON: {e}"))?;
    let merged = merged_schemas(&user);
    serde_json::to_string(&merged).map_err(|e| format!("serialize schemas: {e}"))
}

// =====================================================================
// Internal helpers.
// =====================================================================

fn parse_blocks(blocks_json: &str) -> Result<BTreeMap<String, DocBlock>, String> {
    serde_json::from_str(blocks_json).map_err(|e| format!("invalid blocks JSON: {e}"))
}

fn parse_opts(opts_json: &str) -> Result<EmitOptions, String> {
    if opts_json.trim().is_empty() {
        return Ok(EmitOptions::default());
    }
    serde_json::from_str(opts_json).map_err(|e| format!("invalid opts JSON: {e}"))
}

fn to_js<T>(r: Result<T, String>) -> Result<T, JsError> {
    r.map_err(|e| JsError::new(&e))
}

// =====================================================================
// wasm-bindgen wrappers — only run on wasm32 (JsError imports are not
// available natively). We still expose them natively to avoid duplicating
// exports, but they are not callable.
// =====================================================================

#[wasm_bindgen]
#[must_use]
pub fn version() -> String {
    core_version()
}

#[wasm_bindgen(js_name = evaluateDsl)]
pub fn evaluate_dsl(
    blocks_json: &str,
    template_src: &str,
    schemas_json: &str,
) -> Result<String, JsError> {
    to_js(core_evaluate_dsl(blocks_json, template_src, schemas_json))
}

#[wasm_bindgen(js_name = validateBlocks)]
pub fn validate_blocks(
    blocks_json: &str,
    collisions_json: &str,
    config_json: &str,
) -> Result<String, JsError> {
    to_js(core_validate_blocks(
        blocks_json,
        collisions_json,
        config_json,
    ))
}

#[wasm_bindgen(js_name = emitLlmsTxt)]
pub fn wasm_emit_llms_txt(blocks_json: &str, opts_json: &str) -> Result<String, JsError> {
    to_js(core_emit_llms_txt(blocks_json, opts_json))
}

#[wasm_bindgen(js_name = emitLlmsFull)]
pub fn wasm_emit_llms_full(blocks_json: &str, opts_json: &str) -> Result<String, JsError> {
    to_js(core_emit_llms_full(blocks_json, opts_json))
}

#[wasm_bindgen(js_name = emitSkillMd)]
pub fn wasm_emit_skill_md(blocks_json: &str, opts_json: &str) -> Result<String, JsError> {
    to_js(core_emit_skill_md(blocks_json, opts_json))
}

#[wasm_bindgen(js_name = listSchemas)]
pub fn list_schemas(user_schemas_json: &str) -> Result<String, JsError> {
    to_js(core_list_schemas(user_schemas_json))
}

#[cfg(test)]
mod tests {
    //! Unit tests on `core_*` (native, without wasm-bindgen runtime).
    use super::*;
    use standardoc_core::model::{BlockOrigin, CommentStyle, DocBlock, DocKey, DocMeta};
    use std::path::PathBuf;

    fn sample_blocks() -> BTreeMap<String, DocBlock> {
        let mut tags: BTreeMap<String, Vec<Vec<String>>> = BTreeMap::new();
        tags.insert("doc".to_owned(), vec![vec!["demo.greet".to_owned()]]);
        tags.insert(
            "description".to_owned(),
            vec![vec!["Greets the user".to_owned()]],
        );

        let block = DocBlock {
            key: DocKey::new("demo.greet"),
            label: "greet".to_owned(),
            origin: BlockOrigin::Annotated,
            tags,
            symbol: None,
            meta: DocMeta {
                path: PathBuf::from("src/lib.rs"),
                line_start: 1,
                line_end: 3,
                column: 1,
                file_ext: "rs".to_owned(),
                comment_style: CommentStyle::DocSingle,
                last_indexed: 0,
                source_mtime: 0,
            },
            body_hash: 0,
            diagnostics: vec![],
            virtual_tags: BTreeMap::new(),
            virtual_confidence: None,
            virtual_sources: Vec::new(),
        };

        let mut map = BTreeMap::new();
        map.insert("demo.greet".to_owned(), block);
        map
    }

    fn sample_blocks_json() -> String {
        serde_json::to_string(&sample_blocks()).expect("serialize sample blocks")
    }

    #[test]
    fn version_is_present() {
        assert!(!core_version().is_empty());
    }

    #[test]
    fn evaluate_dsl_renders_block_field() {
        let out = core_evaluate_dsl(
            &sample_blocks_json(),
            "{{ @doc.demo.greet:description }}",
            "{}",
        )
        .unwrap();
        assert!(out.contains("Greets the user"));
    }

    #[test]
    fn evaluate_dsl_reports_eval_errors_on_missing_block() {
        // Reference to a missing block -> evaluation error.
        let res = core_evaluate_dsl(
            &sample_blocks_json(),
            "{{ @doc.nonexistent.key:description }}",
            "{}",
        );
        assert!(res.is_err(), "expected eval error, got {res:?}");
    }

    #[test]
    fn validate_blocks_returns_array() {
        let json = core_validate_blocks(&sample_blocks_json(), "[]", "{}").unwrap();
        let diags: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(diags.is_array());
    }

    #[test]
    fn emit_llms_txt_contains_project_name() {
        let out = core_emit_llms_txt(&sample_blocks_json(), r#"{"projectName": "Demo"}"#).unwrap();
        assert!(out.contains("Demo"));
    }

    #[test]
    fn emit_skill_md_starts_with_front_matter() {
        let out = core_emit_skill_md(&sample_blocks_json(), "{}").unwrap();
        assert!(out.starts_with("---"));
    }

    #[test]
    fn list_schemas_includes_builtins() {
        let out = core_list_schemas("{}").unwrap();
        // At minimum, `param` and `description` tags are built-ins.
        assert!(out.contains("\"param\""), "missing param schema in {out}");
        assert!(out.contains("\"description\""));
    }

    #[test]
    fn invalid_json_returns_error() {
        assert!(core_evaluate_dsl("not json", "x", "{}").is_err());
        assert!(core_validate_blocks("{", "[]", "{}").is_err());
        assert!(core_emit_llms_txt("{nope}", "{}").is_err());
    }

    #[test]
    fn empty_opts_string_uses_defaults() {
        let out = core_emit_llms_txt(&sample_blocks_json(), "").unwrap();
        assert!(out.contains("Project"));
    }

    #[test]
    fn emit_llms_full_smoke() {
        let out = core_emit_llms_full(&sample_blocks_json(), "{}").unwrap();
        assert!(!out.is_empty());
    }
}

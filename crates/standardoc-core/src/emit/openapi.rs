//! `OpenAPI` 3.0 specification emitter.
//!
//! Converts blocks annotated with `@route METHOD PATH` into the
//! `paths.<path>.<method>` of the spec. Other recognized tags:
//! - `@param NAME TYPE description` → `parameters[]` (`in: query` by default,
//!   or `path` if `{NAME}` appears in the `PATH`)
//! - `@response CODE description` → `responses[CODE]`
//! - `@request_body TYPE description` → `requestBody`
//!
//! Blocks without `@route` are ignored. Projects that don't use these tags
//! get an empty `OpenAPI` spec (`paths: {}`) — expected behavior for a
//! project that doesn't expose an HTTP API.
//!
//! Non-standard but minimalist tag convention: we don't invent a fancy
//! schema, we map directly to existing tags. The user can enrich post-hoc
//! with a dedicated `OpenAPI` tool if needed.

use crate::model::DocBlock;
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

/// `OpenAPI` generation options. All fields have sensible defaults — `None`
/// values are substituted with stubs.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenApiOptions {
    /// Titre de l'API (header `info.title`). Default `"API"`.
    pub title: Option<String>,
    /// Version de l'API (header `info.version`). Default `"0.1.0"`.
    pub version: Option<String>,
    /// Description longue de l'API (header `info.description`).
    pub description: Option<String>,
    /// `servers[]` — base URLs where the API is served.
    #[serde(default)]
    pub servers: Vec<String>,
}

/// Builds an `OpenAPI` 3.0 spec from blocks annotated with `@route`.
#[must_use]
pub fn emit_openapi(blocks: &BTreeMap<String, DocBlock>, opts: &OpenApiOptions) -> Value {
    let mut paths: Map<String, Value> = Map::new();

    for block in blocks.values() {
        let Some(operation) = block_to_operation(block) else {
            continue;
        };
        let path_entry = paths
            .entry(operation.path.clone())
            .or_insert_with(|| json!({}));
        if let Some(obj) = path_entry.as_object_mut() {
            obj.insert(operation.method.clone(), operation.spec);
        }
    }

    let mut info = Map::new();
    info.insert(
        "title".to_owned(),
        Value::String(opts.title.clone().unwrap_or_else(|| "API".to_owned())),
    );
    info.insert(
        "version".to_owned(),
        Value::String(opts.version.clone().unwrap_or_else(|| "0.1.0".to_owned())),
    );
    if let Some(desc) = &opts.description {
        info.insert("description".to_owned(), Value::String(desc.clone()));
    }

    let mut spec = Map::new();
    spec.insert("openapi".to_owned(), Value::String("3.0.3".to_owned()));
    spec.insert("info".to_owned(), Value::Object(info));
    if !opts.servers.is_empty() {
        let servers: Vec<Value> = opts
            .servers
            .iter()
            .map(|url| json!({ "url": url }))
            .collect();
        spec.insert("servers".to_owned(), Value::Array(servers));
    }
    spec.insert("paths".to_owned(), Value::Object(paths));
    Value::Object(spec)
}

struct Operation {
    path: String,
    method: String,
    spec: Value,
}

/// Extracts `@route METHOD PATH` from the block. Returns `None` when the
/// `@route` annotation is missing (the block isn't an HTTP endpoint).
fn block_to_operation(block: &DocBlock) -> Option<Operation> {
    let route = block.tags.get("route")?.first()?;
    let method = route.first()?.to_ascii_lowercase();
    let path = route.get(1)?.clone();

    let summary = block.label.clone();
    let description = block
        .tags
        .get("description")
        .and_then(|v| v.first())
        .and_then(|f| f.first())
        .cloned();

    let parameters = build_parameters(block, &path);
    let responses = build_responses(block);
    let request_body = build_request_body(block);

    let mut op = Map::new();
    op.insert("summary".to_owned(), Value::String(summary));
    op.insert(
        "operationId".to_owned(),
        Value::String(block.key.as_str().to_owned()),
    );
    if let Some(desc) = description {
        op.insert("description".to_owned(), Value::String(desc));
    }
    if !parameters.is_empty() {
        op.insert("parameters".to_owned(), Value::Array(parameters));
    }
    if let Some(body) = request_body {
        op.insert("requestBody".to_owned(), body);
    }
    op.insert("responses".to_owned(), Value::Object(responses));

    Some(Operation {
        path,
        method,
        spec: Value::Object(op),
    })
}

/// Builds the `parameters[]` from the block's `@param NAME TYPE
/// description` tags. Heuristic for `in`: if `{NAME}` appears in `path`,
/// it's a path param; otherwise query. No `header` / `cookie` support yet
/// — manually overridable post-export.
fn build_parameters(block: &DocBlock, path: &str) -> Vec<Value> {
    let Some(params) = block.tags.get("param") else {
        return Vec::new();
    };
    params
        .iter()
        .filter_map(|fields| {
            let name = fields.first()?;
            let ty = fields.get(1).cloned();
            let desc = fields.get(2).cloned();
            let placeholder = format!("{{{name}}}");
            let location = if path.contains(&placeholder) {
                "path"
            } else {
                "query"
            };
            let mut p = Map::new();
            p.insert("name".to_owned(), Value::String(name.clone()));
            p.insert("in".to_owned(), Value::String(location.to_owned()));
            // Path params are **always** required in OpenAPI.
            if location == "path" {
                p.insert("required".to_owned(), Value::Bool(true));
            }
            if let Some(d) = desc {
                p.insert("description".to_owned(), Value::String(d));
            }
            if let Some(t) = ty {
                p.insert(
                    "schema".to_owned(),
                    json!({ "type": map_type_to_openapi(&t) }),
                );
            }
            Some(Value::Object(p))
        })
        .collect()
}

/// Builds the `requestBody` from `@request_body TYPE description`. Returns
/// `None` when the tag is missing or malformed.
fn build_request_body(block: &DocBlock) -> Option<Value> {
    let body = block.tags.get("request_body")?.first()?;
    let ty = body.first()?;
    let desc = body.get(1).cloned();
    let mut content = Map::new();
    content.insert(
        "application/json".to_owned(),
        json!({ "schema": { "type": map_type_to_openapi(ty) } }),
    );
    let mut rb = Map::new();
    rb.insert("required".to_owned(), Value::Bool(true));
    if let Some(d) = desc {
        rb.insert("description".to_owned(), Value::String(d));
    }
    rb.insert("content".to_owned(), Value::Object(content));
    Some(Value::Object(rb))
}

/// Builds the `responses` map from `@response CODE description` tags.
/// When none are annotated we add a minimal `200` to stay valid `OpenAPI`
/// (the spec requires at least one response per operation).
fn build_responses(block: &DocBlock) -> Map<String, Value> {
    let mut responses: Map<String, Value> = Map::new();
    if let Some(annotated) = block.tags.get("response") {
        for fields in annotated {
            let Some(code) = fields.first() else { continue };
            let desc = fields
                .get(1)
                .cloned()
                .unwrap_or_else(|| "Response".to_owned());
            responses.insert(code.clone(), json!({ "description": desc }));
        }
    }
    if responses.is_empty() {
        responses.insert(
            "200".to_owned(),
            json!({ "description": "Successful response" }),
        );
    }
    responses
}

/// Pragmatic type-name → `OpenAPI` primitive mapping. We stay
/// conservative — complex types become `object`. The user post-processes
/// with a dedicated tool to describe richer schemas.
fn map_type_to_openapi(ty: &str) -> &'static str {
    let lower = ty.trim().trim_start_matches('&').to_ascii_lowercase();
    match lower.as_str() {
        "i8" | "i16" | "i32" | "i64" | "u8" | "u16" | "u32" | "u64" | "usize" | "isize" | "int"
        | "integer" | "long" | "short" => "integer",
        "f32" | "f64" | "float" | "double" | "number" => "number",
        "bool" | "boolean" => "boolean",
        "string" | "str" | "&str" => "string",
        _ => "object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        BlockOrigin, CommentStyle, DocKey, DocMeta, References, SymbolInfo, SymbolKind, Visibility,
    };
    use std::path::PathBuf;

    fn block(key: &str, tags: BTreeMap<String, Vec<Vec<String>>>) -> DocBlock {
        DocBlock {
            key: DocKey::new(key),
            label: key.to_owned(),
            origin: BlockOrigin::Annotated,
            tags,
            symbol: Some(SymbolInfo {
                kind: SymbolKind::Function,
                visibility: Visibility::Public,
                signature: format!("fn {key}"),
                params: vec![],
                returns: None,
                generics: vec![],
                decorators: vec![],
                is_async: false,
                is_deprecated: false,
                references: References::default(),
            }),
            meta: DocMeta {
                path: PathBuf::from("src/lib.rs"),
                line_start: 1,
                line_end: 1,
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
        }
    }

    fn tags(pairs: &[(&str, Vec<Vec<&str>>)]) -> BTreeMap<String, Vec<Vec<String>>> {
        let mut out = BTreeMap::new();
        for (k, v) in pairs {
            let occurrences: Vec<Vec<String>> = v
                .iter()
                .map(|fields| fields.iter().map(|s| (*s).to_owned()).collect())
                .collect();
            out.insert((*k).to_owned(), occurrences);
        }
        out
    }

    #[test]
    fn emits_minimal_spec_with_no_routes() {
        let blocks = BTreeMap::new();
        let spec = emit_openapi(&blocks, &OpenApiOptions::default());
        assert_eq!(spec["openapi"], "3.0.3");
        assert_eq!(spec["info"]["title"], "API");
        assert_eq!(spec["info"]["version"], "0.1.0");
        assert!(spec["paths"].as_object().unwrap().is_empty());
    }

    #[test]
    fn emits_simple_get_route() {
        let mut blocks = BTreeMap::new();
        blocks.insert(
            "users.list".to_owned(),
            block(
                "users.list",
                tags(&[
                    ("route", vec![vec!["GET", "/users"]]),
                    ("description", vec![vec!["List all users"]]),
                ]),
            ),
        );
        let spec = emit_openapi(&blocks, &OpenApiOptions::default());
        let op = &spec["paths"]["/users"]["get"];
        assert_eq!(op["operationId"], "users.list");
        assert_eq!(op["description"], "List all users");
        assert_eq!(op["responses"]["200"]["description"], "Successful response");
    }

    #[test]
    fn detects_path_param_from_braces_in_route() {
        let mut blocks = BTreeMap::new();
        blocks.insert(
            "users.get".to_owned(),
            block(
                "users.get",
                tags(&[
                    ("route", vec![vec!["GET", "/users/{id}"]]),
                    ("param", vec![vec!["id", "i64", "user id"]]),
                ]),
            ),
        );
        let spec = emit_openapi(&blocks, &OpenApiOptions::default());
        let params = spec["paths"]["/users/{id}"]["get"]["parameters"]
            .as_array()
            .unwrap();
        assert_eq!(params.len(), 1);
        assert_eq!(params[0]["in"], "path");
        assert_eq!(params[0]["required"], true);
        assert_eq!(params[0]["schema"]["type"], "integer");
    }

    #[test]
    fn query_param_when_not_in_path() {
        let mut blocks = BTreeMap::new();
        blocks.insert(
            "users.search".to_owned(),
            block(
                "users.search",
                tags(&[
                    ("route", vec![vec!["GET", "/users"]]),
                    ("param", vec![vec!["q", "string", "search query"]]),
                ]),
            ),
        );
        let spec = emit_openapi(&blocks, &OpenApiOptions::default());
        let p = &spec["paths"]["/users"]["get"]["parameters"][0];
        assert_eq!(p["in"], "query");
        assert!(p.get("required").is_none());
        assert_eq!(p["schema"]["type"], "string");
    }

    #[test]
    fn collects_response_codes() {
        let mut blocks = BTreeMap::new();
        blocks.insert(
            "users.create".to_owned(),
            block(
                "users.create",
                tags(&[
                    ("route", vec![vec!["POST", "/users"]]),
                    (
                        "response",
                        vec![vec!["201", "User created"], vec!["400", "Bad request"]],
                    ),
                ]),
            ),
        );
        let spec = emit_openapi(&blocks, &OpenApiOptions::default());
        let r = &spec["paths"]["/users"]["post"]["responses"];
        assert_eq!(r["201"]["description"], "User created");
        assert_eq!(r["400"]["description"], "Bad request");
    }

    #[test]
    fn request_body_with_type() {
        let mut blocks = BTreeMap::new();
        blocks.insert(
            "users.create".to_owned(),
            block(
                "users.create",
                tags(&[
                    ("route", vec![vec!["POST", "/users"]]),
                    ("request_body", vec![vec!["User", "the user to create"]]),
                ]),
            ),
        );
        let spec = emit_openapi(&blocks, &OpenApiOptions::default());
        let body = &spec["paths"]["/users"]["post"]["requestBody"];
        assert_eq!(body["required"], true);
        assert_eq!(body["description"], "the user to create");
        assert!(body["content"]["application/json"]["schema"].is_object());
    }

    #[test]
    fn options_apply_to_info_and_servers() {
        let blocks = BTreeMap::new();
        let opts = OpenApiOptions {
            title: Some("Matchigo API".to_owned()),
            version: Some("2.0.0".to_owned()),
            description: Some("Pattern matching as a service".to_owned()),
            servers: vec!["https://api.matchigo.dev".to_owned()],
        };
        let spec = emit_openapi(&blocks, &opts);
        assert_eq!(spec["info"]["title"], "Matchigo API");
        assert_eq!(spec["info"]["version"], "2.0.0");
        assert_eq!(spec["info"]["description"], "Pattern matching as a service");
        assert_eq!(spec["servers"][0]["url"], "https://api.matchigo.dev");
    }
}

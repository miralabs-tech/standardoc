//! Synthesize virtual `@doc` annotations from AST + naming conventions.
//!
//! When a project has never been annotated, MCP would return only bare AST
//! signatures — useful but empty for an agent trying to "understand" the
//! codebase. This pass enriches each `DocBlock` with virtual `tags` derived
//! from naming conventions, type signatures, and module structure.
//!
//! Virtual content lives in `DocBlock.virtual_tags`, never mixed with real
//! `tags`. Consumers (MCP `get_doc`, web UI, agents) decide how to merge or
//! display them. UI tooling can offer "Materialize" to write the inferred
//! tags back into source as `///` comments.
//!
//! Aggressiveness is configurable via [`crate::config::VirtualAnnotationsLevel`]:
//! `Off` skips entirely, `Low` covers only public symbols with the highest-
//! confidence templates, `Medium` (default) adds verb-prefix conventions
//! plus param/return narratives, `High` extends to crate-private symbols and
//! emits module-path categorization.

use crate::config::VirtualAnnotationsLevel;
use crate::model::{
    DocBlock, DocKey, ParamInfo, SymbolInfo, SymbolKind, TagFields, TagName, VirtualConfidence,
    Visibility,
};
use std::collections::{BTreeMap, HashSet};
use std::path::Path;

/// Enrich `block` with virtual annotations in-place.
///
/// Idempotent: if `virtual_tags` is already populated, this overwrites it
/// with the freshly computed result. Real `tags` are never touched — virtual
/// content is only emitted for fields that are not already covered by a
/// real annotation, so `@doc` always wins.
pub fn synthesize(block: &mut DocBlock, level: VirtualAnnotationsLevel) {
    if matches!(level, VirtualAnnotationsLevel::Off) {
        return;
    }

    let symbol = match block.symbol.clone() {
        Some(s) => s,
        None => return, // No AST data → nothing to synthesize from.
    };

    if !visibility_allowed(symbol.visibility, level) {
        return;
    }

    let mut virtual_tags: BTreeMap<TagName, Vec<TagFields>> = BTreeMap::new();
    let mut sources: Vec<&'static str> = Vec::new();
    let mut max_confidence = VirtualConfidence::Low;

    if !has_real_description(block) {
        if let Some((desc, src, conf)) = infer_description(&symbol, &block.label, &block.key) {
            virtual_tags
                .entry("description".to_owned())
                .or_default()
                .push(vec![desc]);
            sources.push(src);
            max_confidence = max_confidence.max(conf);
        }
    }

    if matches!(
        level,
        VirtualAnnotationsLevel::Medium | VirtualAnnotationsLevel::High
    ) {
        let real_param_names: HashSet<&str> = block
            .tags
            .get("param")
            .map(|v| {
                v.iter()
                    .filter_map(|t| t.first())
                    .map(String::as_str)
                    .collect()
            })
            .unwrap_or_default();

        let inferred_params = infer_params(&symbol.params);
        let mut emitted_any = false;
        for param_tag in inferred_params {
            let name = param_tag.first().map_or("", String::as_str);
            if name.is_empty() || real_param_names.contains(name) {
                continue;
            }
            virtual_tags
                .entry("param".to_owned())
                .or_default()
                .push(param_tag);
            emitted_any = true;
        }
        if emitted_any {
            sources.push("param-name");
            max_confidence = max_confidence.max(VirtualConfidence::High);
        }

        if !block.tags.contains_key("returns") {
            if let Some(returns_tag) = infer_returns(&symbol) {
                virtual_tags
                    .entry("returns".to_owned())
                    .or_default()
                    .push(returns_tag);
                sources.push("return-type");
                max_confidence = max_confidence.max(VirtualConfidence::High);
            }
        }
    }

    if matches!(level, VirtualAnnotationsLevel::High) {
        if let Some(category) = infer_category_from_path(&block.meta.path) {
            virtual_tags
                .entry("category".to_owned())
                .or_default()
                .push(vec![category]);
            sources.push("module-path");
        }
    }

    if symbol.is_async && !virtual_tags.contains_key("async") {
        virtual_tags
            .entry("async".to_owned())
            .or_default()
            .push(vec![String::new()]);
        sources.push("async-modifier");
    }
    if symbol.is_deprecated && !block.tags.contains_key("deprecated") {
        virtual_tags
            .entry("deprecated".to_owned())
            .or_default()
            .push(vec!["marked deprecated by language attribute".to_owned()]);
        sources.push("deprecated-attribute");
    }

    if virtual_tags.is_empty() {
        block.virtual_tags = BTreeMap::new();
        block.virtual_confidence = None;
        block.virtual_sources = Vec::new();
    } else {
        block.virtual_tags = virtual_tags;
        block.virtual_confidence = Some(max_confidence);
        block.virtual_sources = sources.into_iter().map(str::to_owned).collect();
    }
}

const fn visibility_allowed(vis: Visibility, level: VirtualAnnotationsLevel) -> bool {
    match level {
        VirtualAnnotationsLevel::Off => false,
        VirtualAnnotationsLevel::Low | VirtualAnnotationsLevel::Medium => {
            matches!(vis, Visibility::Public)
        }
        VirtualAnnotationsLevel::High => matches!(
            vis,
            Visibility::Public | Visibility::Crate | Visibility::Internal | Visibility::Inherited
        ),
    }
}

fn has_real_description(block: &DocBlock) -> bool {
    let Some(entries) = block.tags.get("description") else {
        return false;
    };
    entries
        .iter()
        .any(|fields| fields.iter().any(|s| !s.trim().is_empty()))
}

fn infer_description(
    symbol: &SymbolInfo,
    label: &str,
    key: &DocKey,
) -> Option<(String, &'static str, VirtualConfidence)> {
    if matches!(symbol.kind, SymbolKind::Function | SymbolKind::Method) && label == "new" {
        if let Some(p) = parent_segment(key) {
            return Some((
                format!("Creates a new `{p}`."),
                "constructor",
                VirtualConfidence::High,
            ));
        }
    }

    if let Some((from_ty, into_ty, kind)) = parse_from_impl(key.as_str()) {
        let desc = match kind {
            FromKind::From => format!("Converts a `{from_ty}` into a `{into_ty}`."),
            FromKind::TryFrom => format!(
                "Tries to convert a `{from_ty}` into a `{into_ty}`. Returns an error if the conversion is invalid."
            ),
            FromKind::Into => format!("Converts this `{from_ty}` into a `{into_ty}`."),
        };
        return Some((desc, "trait-impl-conversion", VirtualConfidence::High));
    }

    if let Some((trait_short, target)) = parse_trait_impl_method(key.as_str()) {
        match (trait_short.as_str(), label) {
            ("Display", "fmt") => {
                return Some((
                    format!("Formats `{target}` for human-readable display."),
                    "trait-impl-display",
                    VirtualConfidence::High,
                ));
            }
            ("Debug", "fmt") => {
                return Some((
                    format!("Formats `{target}` for debugging output."),
                    "trait-impl-debug",
                    VirtualConfidence::High,
                ));
            }
            ("Default", "default") => {
                return Some((
                    format!("Returns the default-constructed `{target}`."),
                    "trait-impl-default",
                    VirtualConfidence::High,
                ));
            }
            ("Drop", "drop") => {
                return Some((
                    format!("Cleanup hook called when this `{target}` is dropped."),
                    "trait-impl-drop",
                    VirtualConfidence::High,
                ));
            }
            ("Clone", "clone") => {
                return Some((
                    format!("Returns an explicit copy of this `{target}`."),
                    "trait-impl-clone",
                    VirtualConfidence::High,
                ));
            }
            _ => {}
        }
    }

    if matches!(symbol.kind, SymbolKind::Function | SymbolKind::Method) {
        if let Some(rest) = label.strip_prefix("is_") {
            return Some((
                format!("Returns `true` if {}.", humanize_snake(rest)),
                "predicate-is",
                VirtualConfidence::High,
            ));
        }
        if let Some(rest) = label.strip_prefix("has_") {
            return Some((
                format!("Returns `true` if a {} is present.", humanize_snake(rest)),
                "predicate-has",
                VirtualConfidence::High,
            ));
        }
        if let Some(rest) = label.strip_prefix("can_") {
            return Some((
                format!("Returns `true` if {} is allowed.", humanize_snake(rest)),
                "predicate-can",
                VirtualConfidence::High,
            ));
        }
        if let Some(rest) = label.strip_prefix("should_") {
            return Some((
                format!("Returns `true` if {} should occur.", humanize_snake(rest)),
                "predicate-should",
                VirtualConfidence::High,
            ));
        }
    }

    if matches!(symbol.kind, SymbolKind::Function | SymbolKind::Method) && symbol.params.len() <= 1
    {
        match label {
            "len" => {
                return Some((
                    "Returns the number of elements.".to_owned(),
                    "collection-len",
                    VirtualConfidence::High,
                ));
            }
            "is_empty" => {
                return Some((
                    "Returns `true` if the collection contains no elements.".to_owned(),
                    "collection-empty",
                    VirtualConfidence::High,
                ));
            }
            "iter" => {
                return Some((
                    "Returns an iterator over the elements.".to_owned(),
                    "collection-iter",
                    VirtualConfidence::High,
                ));
            }
            "clear" => {
                return Some((
                    "Removes all elements.".to_owned(),
                    "collection-clear",
                    VirtualConfidence::High,
                ));
            }
            _ => {}
        }
    }

    if matches!(symbol.kind, SymbolKind::Function | SymbolKind::Method) {
        const VERB_PREFIXES: &[(&str, &str, &str)] = &[
            ("get_", "Returns the {}.", "verb-get"),
            ("set_", "Sets the {}.", "verb-set"),
            ("create_", "Creates a new {}.", "verb-create"),
            ("delete_", "Deletes the {}.", "verb-delete"),
            ("remove_", "Removes the {}.", "verb-remove"),
            ("find_", "Searches for the {}.", "verb-find"),
            (
                "with_",
                "Returns a copy with the {} configured.",
                "verb-with-builder",
            ),
            ("from_", "Constructs a new instance from a {}.", "verb-from"),
            ("to_", "Converts this value into a {}.", "verb-to"),
            ("as_", "Borrows this value as a {}.", "verb-as"),
            (
                "into_",
                "Consumes this value and returns it as a {}.",
                "verb-into",
            ),
        ];
        for (prefix, template, source) in VERB_PREFIXES {
            if let Some(rest) = label.strip_prefix(prefix) {
                let target = humanize_snake(rest);
                let desc = template.replace("{}", &target);
                return Some((desc, *source, VirtualConfidence::Medium));
            }
        }
        if let Some(rest) = label.strip_prefix("try_") {
            return Some((
                format!(
                    "Fallible variant of `{rest}` — returns an error instead of panicking on failure."
                ),
                "verb-try",
                VirtualConfidence::Medium,
            ));
        }
    }

    match symbol.kind {
        SymbolKind::Struct => Some((
            format!("Struct `{label}` defined in this module."),
            "kind-fallback",
            VirtualConfidence::Low,
        )),
        SymbolKind::Enum => Some((
            format!("Enum `{label}` defined in this module."),
            "kind-fallback",
            VirtualConfidence::Low,
        )),
        SymbolKind::Trait => Some((
            format!("Trait `{label}` defined in this module."),
            "kind-fallback",
            VirtualConfidence::Low,
        )),
        SymbolKind::Interface => Some((
            format!("Interface `{label}` defined in this module."),
            "kind-fallback",
            VirtualConfidence::Low,
        )),
        SymbolKind::TypeAlias => Some((
            format!("Type alias `{label}`."),
            "kind-fallback",
            VirtualConfidence::Low,
        )),
        SymbolKind::Const => Some((
            format!("Constant `{label}`."),
            "kind-fallback",
            VirtualConfidence::Low,
        )),
        SymbolKind::Static => Some((
            format!("Static `{label}`."),
            "kind-fallback",
            VirtualConfidence::Low,
        )),
        SymbolKind::Module => Some((
            format!("Module `{label}`."),
            "kind-fallback",
            VirtualConfidence::Low,
        )),
        SymbolKind::Macro => Some((
            format!("Macro `{label}`."),
            "kind-fallback",
            VirtualConfidence::Low,
        )),
        SymbolKind::Field => Some((
            format!("Field `{label}`."),
            "kind-fallback",
            VirtualConfidence::Low,
        )),
        SymbolKind::Variant => Some((
            format!("Enum variant `{label}`."),
            "kind-fallback",
            VirtualConfidence::Low,
        )),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy)]
enum FromKind {
    From,
    TryFrom,
    Into,
}

/// Detect `<X as From<Y>>`, `<X as TryFrom<Y>>`, `<X as Into<Y>>` patterns
/// from the FQN segment that the language providers embed for trait
/// implementations. Returns `(source_type, target_type, kind)`.
fn parse_from_impl(key: &str) -> Option<(String, String, FromKind)> {
    let segment = key.split('.').find(|s| s.starts_with('<'))?;
    let inner = segment.strip_prefix('<')?.strip_suffix('>')?;
    let (target, trait_part) = inner.split_once(" as ")?;
    let target = target.trim();
    let trait_part = trait_part.trim().replace(' ', "");

    let pick = |prefix: &str| -> Option<String> {
        trait_part
            .strip_prefix(prefix)
            .and_then(|r| r.strip_suffix('>'))
            .map(str::to_owned)
    };
    if let Some(rest) = pick("From<") {
        return Some((rest, target.to_owned(), FromKind::From));
    }
    if let Some(rest) = pick("TryFrom<") {
        return Some((rest, target.to_owned(), FromKind::TryFrom));
    }
    if let Some(rest) = pick("Into<") {
        return Some((target.to_owned(), rest, FromKind::Into));
    }
    None
}

/// Extract `(short_trait_name, target_type)` from `<Target as Path::To::Trait>`
/// segment in an FQN. Whitespace inside the trait path (`std :: fmt :: Display`
/// → `Display`) is normalized.
fn parse_trait_impl_method(key: &str) -> Option<(String, String)> {
    let segment = key.split('.').find(|s| s.starts_with('<'))?;
    let inner = segment.strip_prefix('<')?.strip_suffix('>')?;
    let (target, trait_full) = inner.split_once(" as ")?;
    let target = target.trim().to_owned();
    let trait_short = trait_full
        .trim()
        .replace(' ', "")
        .rsplit("::")
        .next()
        .map_or_else(|| trait_full.trim().to_owned(), str::to_owned);
    Some((trait_short, target))
}

fn parent_segment(key: &DocKey) -> Option<String> {
    let key_str = key.as_str();
    let mut segs = key_str.rsplit('.');
    let _last = segs.next()?;
    segs.next().map(str::to_owned)
}

fn humanize_snake(s: &str) -> String {
    s.replace('_', " ")
}

/// Synthesize `@param NAME TYPE DESCRIPTION` virtual tags for every parameter.
/// Skips `self`/`this` and unnamed placeholders. Description leverages a small
/// dictionary of common parameter conventions, then falls back to a type-driven
/// hint, then to nothing if neither helps.
fn infer_params(params: &[ParamInfo]) -> Vec<TagFields> {
    let mut out = Vec::new();
    for p in params {
        if matches!(
            p.name.as_str(),
            "self" | "&self" | "&mut self" | "this" | "_"
        ) {
            continue;
        }
        let type_repr = p.type_repr.as_deref().unwrap_or("");
        let desc = describe_param(&p.name, type_repr);
        let mut fields: TagFields = vec![p.name.clone()];
        if !type_repr.is_empty() {
            fields.push(type_repr.to_owned());
        }
        if !desc.is_empty() {
            fields.push(desc);
        }
        out.push(fields);
    }
    out
}

fn describe_param(name: &str, type_repr: &str) -> String {
    match name {
        "id" | "uuid" => return "unique identifier".to_owned(),
        "email" => return "email address".to_owned(),
        "password" | "pwd" => return "password (clear text)".to_owned(),
        "name" => return "name".to_owned(),
        "path" => return "filesystem path".to_owned(),
        "url" => return "target URL".to_owned(),
        "uri" => return "URI".to_owned(),
        "req" | "request" => return "incoming request".to_owned(),
        "res" | "response" => return "response object".to_owned(),
        "ctx" | "context" => return "execution context".to_owned(),
        "config" | "cfg" => return "configuration".to_owned(),
        "options" | "opts" => return "options".to_owned(),
        "buf" | "buffer" => return "buffer".to_owned(),
        "data" => return "input data".to_owned(),
        "value" | "v" => return "value".to_owned(),
        "input" => return "input".to_owned(),
        "output" => return "output".to_owned(),
        "key" => return "lookup key".to_owned(),
        "msg" | "message" => return "message".to_owned(),
        _ => {}
    }
    if type_repr.contains("&str") || type_repr.contains("String") {
        return format!("the {} string", humanize_snake(name));
    }
    if type_repr.contains("Path") || type_repr.contains("PathBuf") {
        return format!("filesystem path for {}", humanize_snake(name));
    }
    if type_repr.contains("u32")
        || type_repr.contains("u64")
        || type_repr.contains("usize")
        || type_repr.contains("i32")
        || type_repr.contains("i64")
    {
        return format!("the {} count or value", humanize_snake(name));
    }
    if type_repr.contains("bool") {
        return format!("toggles whether {} is enabled", humanize_snake(name));
    }
    if type_repr.starts_with("Vec<") || type_repr.starts_with("&[") {
        return format!("collection of {}", humanize_snake(name));
    }
    String::new()
}

/// Build `@returns TYPE DESCRIPTION` for known return-type shapes:
/// `Result<T, E>`, `Option<T>`, `impl Iterator<Item = T>`, `bool`,
/// or a generic fallback for anything else non-empty.
fn infer_returns(symbol: &SymbolInfo) -> Option<TagFields> {
    let returns = symbol.returns.as_ref()?;
    let repr = returns.repr.trim();
    if repr.is_empty() || repr == "()" {
        return None;
    }
    if let Some(inside) = repr
        .strip_prefix("Result<")
        .and_then(|r| r.strip_suffix('>'))
    {
        let (ok_ty, err_ty) =
            split_top_level_comma(inside).unwrap_or_else(|| (inside.to_owned(), String::new()));
        let desc = if err_ty.trim().is_empty() {
            "the result on success, or an error if the operation fails".to_owned()
        } else {
            format!(
                "the `{}` on success, or `{}` if the operation fails",
                ok_ty.trim(),
                err_ty.trim()
            )
        };
        return Some(vec![repr.to_owned(), desc]);
    }
    if let Some(inside) = repr
        .strip_prefix("Option<")
        .and_then(|r| r.strip_suffix('>'))
    {
        return Some(vec![
            repr.to_owned(),
            format!("`Some({})` if available, otherwise `None`", inside.trim()),
        ]);
    }
    if repr.contains("Iterator<Item") || repr.starts_with("impl Iterator") {
        return Some(vec![
            repr.to_owned(),
            "iterator over the resulting elements".to_owned(),
        ]);
    }
    if repr == "bool" {
        return Some(vec![repr.to_owned(), "true / false outcome".to_owned()]);
    }
    Some(vec![repr.to_owned(), format!("the resulting `{repr}`")])
}

/// Top-level comma split: respects nested `<…>`, `(…)`, `[…]`. Used to
/// disentangle `Result<T, E>` into ok/err halves.
fn split_top_level_comma(s: &str) -> Option<(String, String)> {
    let mut depth: i32 = 0;
    for (i, c) in s.char_indices() {
        match c {
            '<' | '(' | '[' => depth += 1,
            '>' | ')' | ']' => depth -= 1,
            ',' if depth == 0 => {
                return Some((s[..i].to_owned(), s[i + 1..].to_owned()));
            }
            _ => {}
        }
    }
    None
}

/// First non-trivial path segment becomes the virtual `@category` tag —
/// gives `auth/login.rs` → `auth`, `db/queries/user.rs` → `db`. Skips
/// scaffolding directories (`src`, `lib`, `crates`, `tests`, `examples`).
fn infer_category_from_path(path: &Path) -> Option<String> {
    use std::path::Component;
    let mut segments: Vec<&str> = Vec::new();
    for c in path.components() {
        if let Component::Normal(s) = c {
            if let Some(s) = s.to_str() {
                segments.push(s);
            }
        }
    }
    let candidate = segments.iter().find(|s| {
        !matches!(
            s.to_lowercase().as_str(),
            "src" | "lib" | "crates" | "tests" | "examples"
        )
    })?;
    let trimmed = candidate
        .trim_end_matches(".rs")
        .trim_end_matches(".tsx")
        .trim_end_matches(".ts")
        .trim_end_matches(".jsx")
        .trim_end_matches(".js")
        .trim_end_matches(".py")
        .trim_end_matches(".lua");
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

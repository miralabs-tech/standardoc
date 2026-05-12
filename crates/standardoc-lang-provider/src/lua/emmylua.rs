//! Minimal EmmyLua / LuaCATS annotation parser.
//!
//! Extracts `@param <name> <type> [description]` and `@return <type>
//! [description]` from doc comment text and uses them to enrich a `Signature`
//! whose `Param.ty` would otherwise be empty (Lua is dynamically typed —
//! providers cannot derive types from the AST alone).
//!
//! Day-1 scope:
//! - `@param NAME TYPE [desc]`  → fills `Param.ty.display = TYPE` when the
//!   param's name matches.
//! - `@return TYPE [desc]`       → fills `Signature.returns` (first occurrence
//!   wins; subsequent `@return` tags are joined with `,` to preserve
//!   multi-return semantics in the type string).
//!
//! Out of scope day-1: `@field`, `@class`, `@type` on locals, generic syntax
//! (`@generic T`), union narrowing past raw text capture. The raw doc text
//! always remains available on the document side; this parser is additive.

use standardoc_ir::{Signature, TypeRef};

/// Patch `sig.params[i].ty` for any `i` whose `name` appears as a
/// `@param NAME TYPE` row in `doc_text`. Patches `sig.returns` from
/// `@return TYPE` rows. Returns `true` when at least one field was modified.
///
/// The parser is line-oriented and tolerant: it accepts both `---@param`
/// (LuaCATS style) and bare `@param` (extracted doc text already has the
/// leading `---` stripped by `extract_doc.rs`).
pub(crate) fn enrich_signature(sig: &mut Signature, doc_text: &str) -> bool {
    let mut changed = false;
    let mut returns_acc: Vec<String> = Vec::new();
    for raw in doc_text.lines() {
        let line = raw.trim_start_matches('-').trim();
        if let Some(rest) = strip_tag_prefix(line, "@param") {
            if let Some((name, ty)) = parse_param_row(rest) {
                for p in &mut sig.params {
                    if p.name == name && p.ty.display.is_empty() {
                        p.ty = TypeRef::new(ty.clone());
                        changed = true;
                    }
                }
            }
        } else if let Some(rest) = strip_tag_prefix(line, "@return")
            && let Some(ty) = parse_return_row(rest)
        {
            returns_acc.push(ty);
        }
    }
    if !returns_acc.is_empty() && sig.returns.is_none() {
        sig.returns = Some(TypeRef::new(returns_acc.join(", ")));
        changed = true;
    }
    changed
}

fn strip_tag_prefix<'a>(line: &'a str, tag: &str) -> Option<&'a str> {
    let rest = line.strip_prefix(tag)?;
    // Require whitespace separator between the tag and its body so `@params`
    // (note the s) does not match `@param`.
    let first = rest.chars().next()?;
    if first.is_whitespace() {
        Some(rest.trim_start())
    } else {
        None
    }
}

fn parse_param_row(s: &str) -> Option<(String, String)> {
    // `<name>[?] <type> [description]`. Optional `?` marks an optional param.
    let mut iter = s.splitn(2, char::is_whitespace);
    let name_raw = iter.next()?.trim();
    let rest = iter.next()?.trim();
    if name_raw.is_empty() || rest.is_empty() {
        return None;
    }
    let name = name_raw.trim_end_matches('?').to_string();
    // Type is everything up to the first ` -` or the first space-then-letter
    // that introduces a description. Simplest robust heuristic: take the
    // first whitespace-delimited token as the type. EmmyLua allows complex
    // composite types `string|number|nil` — those are non-space tokens.
    let ty = rest
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_string();
    if ty.is_empty() {
        return None;
    }
    Some((name, ty))
}

fn parse_return_row(s: &str) -> Option<String> {
    let ty = s.split_whitespace().next()?.to_string();
    if ty.is_empty() { None } else { Some(ty) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use standardoc_ir::Param;

    fn sig_with_params(names: &[&str]) -> Signature {
        Signature {
            params: names
                .iter()
                .map(|n| Param {
                    name: (*n).to_string(),
                    ty: TypeRef::new(""),
                    default: None,
                })
                .collect(),
            returns: None,
            modifiers: Default::default(),
            meta: Default::default(),
        }
    }

    #[test]
    fn enriches_param_type_when_name_matches() {
        let mut sig = sig_with_params(&["a", "b"]);
        let changed = enrich_signature(&mut sig, "@param a number\n@param b string\n");
        assert!(changed);
        assert_eq!(sig.params[0].ty.display, "number");
        assert_eq!(sig.params[1].ty.display, "string");
    }

    #[test]
    fn enriches_return_type_when_absent() {
        let mut sig = sig_with_params(&["x"]);
        let changed = enrich_signature(&mut sig, "@return number\n");
        assert!(changed);
        assert_eq!(sig.returns.as_ref().unwrap().display, "number");
    }

    #[test]
    fn multiple_returns_are_joined_with_comma() {
        let mut sig = sig_with_params(&["x"]);
        let changed = enrich_signature(&mut sig, "@return number\n@return string\n");
        assert!(changed);
        assert_eq!(sig.returns.as_ref().unwrap().display, "number, string");
    }

    #[test]
    fn does_not_overwrite_existing_param_type() {
        let mut sig = sig_with_params(&["a"]);
        sig.params[0].ty = TypeRef::new("preset");
        let changed = enrich_signature(&mut sig, "@param a number\n");
        assert!(!changed, "must not overwrite a type that was already set");
        assert_eq!(sig.params[0].ty.display, "preset");
    }

    #[test]
    fn does_not_overwrite_existing_returns() {
        let mut sig = sig_with_params(&["x"]);
        sig.returns = Some(TypeRef::new("preset"));
        let changed = enrich_signature(&mut sig, "@return number\n");
        assert!(!changed);
        assert_eq!(sig.returns.as_ref().unwrap().display, "preset");
    }

    #[test]
    fn tolerates_leading_dashes_from_raw_doc_lines() {
        // Doc text retains source-side `--` prefixes when present (defensive).
        let mut sig = sig_with_params(&["a"]);
        let changed = enrich_signature(&mut sig, "---@param a string\n");
        assert!(changed);
        assert_eq!(sig.params[0].ty.display, "string");
    }

    #[test]
    fn unknown_param_name_is_silently_skipped() {
        let mut sig = sig_with_params(&["a"]);
        let changed = enrich_signature(&mut sig, "@param zzz number\n");
        assert!(!changed);
        assert_eq!(sig.params[0].ty.display, "");
    }

    #[test]
    fn optional_param_marker_stripped_from_name() {
        let mut sig = sig_with_params(&["limit"]);
        let changed = enrich_signature(&mut sig, "@param limit? number\n");
        assert!(changed);
        assert_eq!(sig.params[0].ty.display, "number");
    }

    #[test]
    fn tag_must_be_followed_by_whitespace() {
        // `@params` (typo: trailing s) must not be parsed as `@param`.
        let mut sig = sig_with_params(&["a"]);
        let changed = enrich_signature(&mut sig, "@params a number\n");
        assert!(!changed);
    }

    #[test]
    fn unrelated_lines_are_ignored() {
        let mut sig = sig_with_params(&["a"]);
        let changed = enrich_signature(
            &mut sig,
            "This is a description.\nNot an annotation.\n@param a number\n",
        );
        assert!(changed);
        assert_eq!(sig.params[0].ty.display, "number");
    }
}

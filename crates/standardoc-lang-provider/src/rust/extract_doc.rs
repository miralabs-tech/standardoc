use syn::{AttrStyle, Attribute, Expr, Lit, Meta};

/// Extract the description text from outer doc attributes (`///` or `/** */`,
/// both desugared by rustc to `#[doc = "..."]`). Returns `None` if no `#[doc]`
/// attribute is present, or if all of them are non-string literals.
///
/// Each `///` line becomes its own `#[doc]` attribute; `/** */` blocks become
/// a single `#[doc]` with embedded newlines. We concatenate the string values
/// with `\n`, stripping one leading space per fragment (rustdoc convention).
pub(crate) fn extract_outer(attrs: &[Attribute]) -> Option<String> {
    extract_with(attrs, |a| matches!(a.style, AttrStyle::Outer))
}

/// Extract the description text from inner doc attributes (`//!` or
/// `#![doc = "..."]`). Used for the file-level Module symbol.
pub(crate) fn extract_inner(attrs: &[Attribute]) -> Option<String> {
    extract_with(attrs, |a| matches!(a.style, AttrStyle::Inner(_)))
}

fn extract_with(attrs: &[Attribute], style_pred: impl Fn(&Attribute) -> bool) -> Option<String> {
    let mut lines: Vec<String> = Vec::new();
    for attr in attrs {
        if !style_pred(attr) || !attr.path().is_ident("doc") {
            continue;
        }
        if let Some(text) = doc_value(&attr.meta) {
            lines.push(strip_leading_space(&text));
        }
    }
    if lines.is_empty() {
        None
    } else {
        Some(lines.join("\n"))
    }
}

fn doc_value(meta: &Meta) -> Option<String> {
    let Meta::NameValue(nv) = meta else {
        return None;
    };
    let Expr::Lit(expr_lit) = &nv.value else {
        return None;
    };
    let Lit::Str(s) = &expr_lit.lit else {
        return None;
    };
    Some(s.value())
}

fn strip_leading_space(s: &str) -> String {
    s.strip_prefix(' ').unwrap_or(s).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_attrs(src: &str) -> Vec<Attribute> {
        let file: syn::File = syn::parse_str(src).unwrap();
        match file.items.into_iter().next().unwrap() {
            syn::Item::Fn(f) => f.attrs,
            syn::Item::Struct(s) => s.attrs,
            other => panic!("unexpected item: {other:?}"),
        }
    }

    #[test]
    fn triple_slash_extracts_description() {
        let attrs = parse_attrs("/// Creates a new user.\nfn foo() {}");
        assert_eq!(
            extract_outer(&attrs).as_deref(),
            Some("Creates a new user.")
        );
    }

    #[test]
    fn multiple_slash_lines_join_with_newline() {
        let attrs = parse_attrs("/// First line.\n/// Second line.\nfn foo() {}");
        assert_eq!(
            extract_outer(&attrs).as_deref(),
            Some("First line.\nSecond line.")
        );
    }

    #[test]
    fn block_doc_comment_extracted() {
        let attrs = parse_attrs("/** A struct doc. */\nstruct X;");
        let out = extract_outer(&attrs).unwrap();
        assert!(out.contains("A struct doc."));
    }

    #[test]
    fn explicit_doc_attribute_extracted() {
        let attrs = parse_attrs("#[doc = \"Hello\"]\nfn foo() {}");
        assert_eq!(extract_outer(&attrs).as_deref(), Some("Hello"));
    }

    #[test]
    fn no_doc_returns_none() {
        let attrs = parse_attrs("fn foo() {}");
        assert!(extract_outer(&attrs).is_none());
    }

    #[test]
    fn non_doc_attributes_are_ignored() {
        let attrs = parse_attrs("#[derive(Debug)]\nstruct X;");
        assert!(extract_outer(&attrs).is_none());
    }

    #[test]
    fn leading_space_stripped_per_line() {
        // `/// foo` desugars to `#[doc = " foo"]` with a leading space; we strip it.
        let attrs = parse_attrs("/// foo\nfn bar() {}");
        assert_eq!(extract_outer(&attrs).as_deref(), Some("foo"));
    }

    #[test]
    fn mixed_outer_and_inner_keeps_only_outer() {
        // syn::File-level inner attrs are accessible only via parse_file; here we
        // just sanity-check that AttrStyle::Outer matches don't pull #![doc].
        let file: syn::File =
            syn::parse_str("#![doc = \"inner mod\"]\n/// outer fn doc\nfn foo() {}").unwrap();
        let inner = extract_inner(&file.attrs);
        assert_eq!(inner.as_deref(), Some("inner mod"));
        let fn_attrs = match file.items.into_iter().next().unwrap() {
            syn::Item::Fn(f) => f.attrs,
            _ => panic!(),
        };
        let outer = extract_outer(&fn_attrs);
        assert_eq!(outer.as_deref(), Some("outer fn doc"));
    }

    #[test]
    fn at_tags_kept_inline_in_description() {
        // We do NOT parse JSDoc/Rustdoc tags day-1 (Q2 vote: description-only).
        let attrs = parse_attrs("/// Computes sum.\n/// @param a first\nfn foo() {}");
        assert_eq!(
            extract_outer(&attrs).as_deref(),
            Some("Computes sum.\n@param a first")
        );
    }
}

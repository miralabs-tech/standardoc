//! Shared type extraction for Rust `syn::Type` nodes.
//!
//! Returns the last path segment preserved with its parametric args
//! (`Vec<Foo>`) when the type is a `Path`, stripping through `&`,
//! `&mut`, parens and groups. Returns `None` for tuples, closures,
//! slices, impl-trait, never, and other non-nominal shapes.
//!
//! Reused by:
//!   - `extract_call::local_type_env` (Bug E-3 Phase 1: binding -> type)
//!   - `struct_field_table` (Bug E-3 Phase 1: struct.field -> type)
//!   - `return_type_table` (Bug E-3 ext P-E3.1: fn-fqdn -> return type)
//!   - `extract_call::CallVisitor::type_of_expr` (Bug E-3 Phase 3 + ext
//!     P-E3.2: chained-method receivers via builtin returns and closure
//!     substitution).

use syn::Type;

/// Bug E-3 ext P-E3.2: extract the last path segment preserving
/// angle-bracketed generic args as printed in source. Used by closure-arg
/// substitution to map e.g. `Vec<Foo>` → bind `T = Foo`. Generic args
/// that can't be rendered (lifetimes, const generics, associated-type
/// bindings) collapse to `_`. Callers needing only the nominal head
/// slice via [`nominal_of`].
pub(crate) fn parametric_type(ty: &Type) -> Option<String> {
    match ty {
        Type::Path(p) => p.path.segments.last().map(render_segment),
        Type::Reference(r) => parametric_type(&r.elem),
        Type::Paren(p) => parametric_type(&p.elem),
        Type::Group(g) => parametric_type(&g.elem),
        _ => None,
    }
}

fn render_segment(seg: &syn::PathSegment) -> String {
    let name = seg.ident.to_string();
    match &seg.arguments {
        syn::PathArguments::AngleBracketed(ab) => {
            let parts: Vec<String> = ab.args.iter().map(render_generic_arg).collect();
            format!("{name}<{}>", parts.join(", "))
        }
        // `Fn(_) -> _` parenthesized args are not nominal generics —
        // collapse them out like the `None` case.
        syn::PathArguments::None | syn::PathArguments::Parenthesized(_) => name,
    }
}

fn render_generic_arg(arg: &syn::GenericArgument) -> String {
    match arg {
        syn::GenericArgument::Type(t) => parametric_type(t).unwrap_or_else(|| "_".to_string()),
        _ => "_".to_string(),
    }
}

/// Bug E-3 ext P-E3.2: slice the nominal portion of a parametric type
/// string. `Vec<Foo>` → `Vec`. Returns the input untouched when no `<`
/// is present (bare nominal already).
pub(crate) fn nominal_of(parametric: &str) -> &str {
    parametric
        .find('<')
        .map_or(parametric, |i| &parametric[..i])
}

/// Bug E-3 ext P-E3.2: split a parametric type's generic args at depth
/// zero. `HashMap<String, Vec<u8>>` → `["String", "Vec<u8>"]`. Returns
/// an empty vec for non-parametric types.
pub(crate) fn generic_args(parametric: &str) -> Vec<&str> {
    let Some(start) = parametric.find('<') else {
        return Vec::new();
    };
    let Some(end) = parametric.rfind('>') else {
        return Vec::new();
    };
    if end <= start {
        return Vec::new();
    }
    split_args_depth_zero(&parametric[start + 1..end])
}

fn split_args_depth_zero(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth: i32 = 0;
    let mut start = 0;
    for (i, ch) in s.char_indices() {
        match ch {
            '<' => depth += 1,
            '>' => depth -= 1,
            ',' if depth == 0 => {
                push_trimmed(&mut out, &s[start..i]);
                start = i + ch.len_utf8();
            }
            _ => {}
        }
    }
    push_trimmed(&mut out, &s[start..]);
    out
}

fn push_trimmed<'a>(out: &mut Vec<&'a str>, s: &'a str) {
    let trimmed = s.trim();
    if !trimmed.is_empty() {
        out.push(trimmed);
    }
}

/// Bug E-3 ext P-E3.2: substitute generic-param placeholders in a
/// template string using the receiver's nominal + args. E.g. for receiver
/// `Vec<Foo>` and template `"Iterator<T>"`, returns `"Iterator<Foo>"`.
/// Token rules per receiver nominal:
///   * `Result`: `T` = args[0], `E` = args[1]
///   * `HashMap | BTreeMap`: `K` = args[0], `V` = args[1]
///   * any other nominal: `T` = args[0] — the permissive catch-all
///     covering every single-type-param container (`Vec`, `Option`,
///     `Box`, `Arc`, `Iterator`, `Rc`, `Cell`, `RefCell`, `Mutex`, …).
/// Stops short of full type-param resolution — that's E-3.3.
pub(crate) fn substitute_template(
    template: &str,
    parent_nominal: &str,
    parent_args: &[&str],
) -> String {
    let bindings: Vec<(&str, &str)> = match parent_nominal {
        "Result" => parent_args
            .iter()
            .take(2)
            .enumerate()
            .map(|(i, a)| if i == 0 { ("T", *a) } else { ("E", *a) })
            .collect(),
        "HashMap" | "BTreeMap" => parent_args
            .iter()
            .take(2)
            .enumerate()
            .map(|(i, a)| if i == 0 { ("K", *a) } else { ("V", *a) })
            .collect(),
        _ => parent_args
            .first()
            .map(|a| vec![("T", *a)])
            .unwrap_or_default(),
    };
    substitute_tokens(template, &bindings)
}

fn substitute_tokens(s: &str, bindings: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let ch = bytes[i];
        if ch.is_ascii_alphabetic() || ch == b'_' {
            let start = i;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            let token = &s[start..i];
            if let Some((_, v)) = bindings.iter().find(|(k, _)| *k == token) {
                out.push_str(v);
            } else if matches!(token, "T" | "E" | "K" | "V") {
                // Bug E-3.3: unbound substitution placeholders collapse
                // to `_` so they don't leak as literal type-param names
                // (`"T"`, `"E"`, …) into downstream `receiver_type`
                // columns or closure bindings.
                out.push('_');
            } else {
                out.push_str(token);
            }
        } else {
            // Copy the whole UTF-8 char rather than the raw byte, so a
            // multi-byte char survives instead of being split into Latin-1
            // bytes. `i` always sits on a char boundary here (the identifier
            // branch only steps over ASCII).
            let c = s[i..].chars().next().unwrap_or('\u{FFFD}');
            out.push(c);
            i += c.len_utf8();
        }
    }
    out
}

#[cfg(test)]
mod tests;

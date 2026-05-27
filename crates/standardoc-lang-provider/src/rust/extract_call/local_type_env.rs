//! Bug E-3 Phase 1: receiver-type environment for a single Rust fn body.
//!
//! Captures bindings → nominal type names so `visit_expr_method_call`
//! can annotate emitted CALLS edges with `receiver_type`. Phase 1 is
//! deliberately flat (no nested-block scoping); shadowings inside nested
//! blocks may yield false positives, accepted per ADR. Phase 3 may
//! revisit with proper scoping if measured noise warrants it.
//!
//! Inference sources covered:
//!   * fn params with type annotation (`fn f(x: &Foo)` → `x : Foo`)
//!   * annotated lets (`let x: Vec<u8> = ...` → `x : Vec`)
//!   * constructor lets (`let x = Vec::new()` / `String::from(...)` /
//!     `T::default()` → `x : Vec` / `String` / `T`)
//!
//! Out of scope Phase 1: tuple/struct destructuring, match-arm bindings,
//! closures, return-type inference, chained method calls.

use std::collections::HashMap;

use syn::punctuated::Punctuated;
use syn::{Expr, FnArg, Local, Pat, Token, Type};

#[derive(Default, Debug)]
pub(super) struct LocalTypeEnv {
    bindings: HashMap<String, String>,
}

impl LocalTypeEnv {
    pub(super) fn from_fn_params(inputs: &Punctuated<FnArg, Token![,]>) -> Self {
        let mut env = Self::default();
        for input in inputs {
            if let FnArg::Typed(pt) = input
                && let Pat::Ident(pi) = &*pt.pat
                && let Some(t) = nominal_type(&pt.ty)
            {
                env.bindings.insert(pi.ident.to_string(), t);
            }
        }
        env
    }

    /// Record `let <name> [: <Type>] [= <init>]` into the env. Annotated
    /// type wins over the init; non-ident patterns (destructuring) are
    /// skipped per Phase 1 scope.
    pub(super) fn record_local(&mut self, local: &Local) {
        let (name, annotated) = match &local.pat {
            Pat::Type(pt) => {
                let Pat::Ident(pi) = &*pt.pat else {
                    return;
                };
                (pi.ident.to_string(), nominal_type(&pt.ty))
            }
            Pat::Ident(pi) => (pi.ident.to_string(), None),
            _ => return,
        };
        let ty = annotated.or_else(|| {
            local
                .init
                .as_ref()
                .and_then(|init| type_from_init_expr(&init.expr))
        });
        if let Some(t) = ty {
            self.bindings.insert(name, t);
        }
    }

    // Used by tests + Bug E-3 P1.4 (visit_expr_method_call). The lib
    // build alone has no caller until P1.4 lands; until then, allow.
    #[allow(dead_code)]
    pub(super) fn lookup(&self, name: &str) -> Option<&str> {
        self.bindings.get(name).map(String::as_str)
    }
}

/// Last path segment of a `syn::Type` when it's a `Path` (optionally
/// behind references). Drops generic args. Returns `None` for tuples,
/// closures, slices, impl-trait and other non-nominal shapes.
fn nominal_type(ty: &Type) -> Option<String> {
    match ty {
        Type::Path(p) => p.path.segments.last().map(|s| s.ident.to_string()),
        Type::Reference(r) => nominal_type(&r.elem),
        Type::Paren(p) => nominal_type(&p.elem),
        Type::Group(g) => nominal_type(&g.elem),
        _ => None,
    }
}

/// Infer the binding's nominal type from a constructor-shaped initializer.
/// Recognises `<Type>::<ctor>(...)` where `<ctor>` is a whitelisted
/// constructor name. Returns the type segment immediately before the ctor.
fn type_from_init_expr(expr: &Expr) -> Option<String> {
    // Strip references / parens so `&Vec::new()` still matches.
    let expr = unwrap_expr(expr);
    let Expr::Call(call) = expr else { return None };
    let Expr::Path(p) = &*call.func else {
        return None;
    };
    let segs = &p.path.segments;
    if segs.len() < 2 {
        return None;
    }
    let ctor = segs.last()?.ident.to_string();
    if !is_known_constructor(&ctor) {
        return None;
    }
    segs.iter().rev().nth(1).map(|s| s.ident.to_string())
}

fn unwrap_expr(mut expr: &Expr) -> &Expr {
    loop {
        match expr {
            Expr::Reference(r) => expr = &r.expr,
            Expr::Paren(p) => expr = &p.expr,
            Expr::Group(g) => expr = &g.expr,
            _ => return expr,
        }
    }
}

const fn is_known_constructor(name: &str) -> bool {
    matches!(
        name.as_bytes(),
        b"new"
            | b"default"
            | b"from"
            | b"from_str"
            | b"from_iter"
            | b"with_capacity"
            | b"with_value"
            | b"try_from"
            | b"parse"
            | b"build"
            | b"open"
            | b"create"
            | b"init"
    )
}

#[cfg(test)]
mod tests;

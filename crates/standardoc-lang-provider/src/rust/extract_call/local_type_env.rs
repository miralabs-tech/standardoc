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
//!
//! Bug E-3 ext P-E3.2: bindings now store *parametric* type strings
//! (`"Vec<Foo>"`) instead of nominal-only — closure-arg substitution
//! needs the generic args. Callers that only need the nominal portion
//! slice via [`crate::rust::type_name::nominal_of`]. A push/pop stack
//! of [`ClosureScope`] frames sits on top so `.map(|x| ...)` etc. can
//! bind closure-locals to inferred types only within the closure body.

use std::collections::HashMap;

use syn::punctuated::Punctuated;
use syn::{Expr, FnArg, Local, Pat, Token};

use crate::rust::type_name::parametric_type;

#[derive(Default, Debug)]
pub(super) struct LocalTypeEnv {
    bindings: HashMap<String, String>,
    /// Bug E-3 ext P-E3.2: stack of per-closure binding frames pushed by
    /// `visit_expr_method_call` for closure args whose receiver +
    /// method are annotated in the builtin registry. Top of stack
    /// shadows bindings; popped on closure exit. Nested closures stack
    /// naturally.
    closure_scopes: Vec<HashMap<String, String>>,
}

impl LocalTypeEnv {
    pub(super) fn from_fn_params(inputs: &Punctuated<FnArg, Token![,]>) -> Self {
        let mut env = Self::default();
        for input in inputs {
            if let FnArg::Typed(pt) = input
                && let Pat::Ident(pi) = &*pt.pat
                && let Some(t) = parametric_type(&pt.ty)
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
                (pi.ident.to_string(), parametric_type(&pt.ty))
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

    /// Bug E-3 Phase 1: nominal-or-parametric lookup. Walks closure
    /// scopes top-down (innermost first) then falls back to the flat
    /// per-fn bindings. Returns the parametric form (`"Vec<Foo>"`) when
    /// available; callers slice with `nominal_of` for nominal-only use.
    pub(super) fn lookup(&self, name: &str) -> Option<&str> {
        for scope in self.closure_scopes.iter().rev() {
            if let Some(t) = scope.get(name) {
                return Some(t.as_str());
            }
        }
        self.bindings.get(name).map(String::as_str)
    }

    /// Bug E-3 ext P-E3.2: push a closure-arg frame. Each entry maps a
    /// closure-local ident (from the closure's input pattern) to its
    /// inferred parametric type. Frames stack — pop on closure exit.
    pub(super) fn push_closure_scope(&mut self, frame: HashMap<String, String>) {
        self.closure_scopes.push(frame);
    }

    pub(super) fn pop_closure_scope(&mut self) {
        self.closure_scopes.pop();
    }

    /// Bug E-3 ext P-E3.2.3: directly record a binding from an
    /// out-of-band inference source (e.g. workspace return-type table
    /// fallback for `let x = workspace_fn()`). Use when [`record_local`]
    /// returns without binding because the init isn't a known
    /// constructor shape.
    pub(super) fn set_binding(&mut self, name: String, ty: String) {
        self.bindings.insert(name, ty);
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

//! Template-extraction subsystem for Vue and Svelte SFCs.
//!
//! Lock 41 §1 Q4 (PIVOT) extracts identifier references from `<template>`
//! day-1 across Vue, Svelte and React JSX so the cross-framework graph
//! stays symmetric. Vue and Svelte share a state-machine-based template
//! parser with framework-specific directive vocabularies; React JSX is
//! handled inline by `TsProvider::JsxRefVisitor` because `.tsx`/`.jsx`
//! is parseable as a top-level TS/JS module.
//!
//! Each parser produces a flat list of [`TemplateRef`] records that the
//! Vue/Svelte SFC orchestrator turns into `RawEdge` instances tagged with
//! the appropriate `attributes` from the lock 41 §2.6 vocabulary
//! (`template-event`, `template-bind`, `template-interpolation`,
//! `template-directive`, `template-component-ref`).

use swc_core::common::sync::Lrc;
use swc_core::common::{FileName, SourceMap};
use swc_core::ecma::ast::{EsVersion, Ident};
use swc_core::ecma::parser::{EsSyntax, Parser, StringInput, Syntax, lexer::Lexer};
use swc_core::ecma::visit::{Visit, VisitWith};

pub(crate) mod svelte;
pub(crate) mod vue;

/// One identifier reference extracted from a template region.
///
/// `name` carries the raw identifier text as it appeared in the template.
/// Resolution to a `ResolvedOrUnresolved::{Resolved,Unresolved}` happens
/// at the SFC orchestrator layer because that's where the surrounding
/// `<script>` import table is in scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TemplateRef {
    /// Identifier text (left-most segment of a member expression for
    /// `obj.prop` patterns — same convention as TsProvider call extraction).
    pub name: String,
    /// One of the `template-*` slugs from lock 41 §2.6. Caller forwards
    /// this verbatim into `RawEdge.attributes`.
    pub attribute: TemplateAttribute,
    /// Byte offset of the reference within the original SFC source.
    /// Used by the orchestrator to populate `Site { line, col }` after
    /// running the offset → line/col conversion.
    pub byte_offset: usize,
}

/// Closed enum of template attribute slugs (lock 41 §2.6).
///
/// Closed-by-design: each variant maps to a stable string in
/// [`Self::as_str`]. New slugs require a code change here — symmetric
/// with `EdgeKind` in the IR crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum TemplateAttribute {
    /// `@click` / `on:click` / `onClick` / `on:event`.
    Event,
    /// `:prop` / `v-bind:prop` / `bind:prop` / JSX `prop={...}`.
    Bind,
    /// `{{ expr }}` / `{expr}` / JSX child `{expr}`.
    Interpolation,
    /// `v-if` / `v-for` / `v-show` / `{#if}` / `{#each}` / `{:else}`.
    Directive,
    /// `<MyComp />` element-name reference (uppercase first letter).
    ComponentRef,
}

impl TemplateAttribute {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Event => "template-event",
            Self::Bind => "template-bind",
            Self::Interpolation => "template-interpolation",
            Self::Directive => "template-directive",
            Self::ComponentRef => "template-component-ref",
        }
    }
}

/// Visitor abstraction shared by the Vue and Svelte template parsers.
///
/// Day-1 implementation will likely just collect into a `Vec<TemplateRef>`
/// — the trait exists so phase-B impl can swap a streaming sink for tests
/// without rewriting the state machines.
pub(crate) trait TemplateRefSink {
    fn push(&mut self, r: TemplateRef);
}

impl TemplateRefSink for Vec<TemplateRef> {
    fn push(&mut self, r: TemplateRef) {
        Self::push(self, r);
    }
}

/// Identifier-extraction helper invoked by the per-framework parsers on
/// each individual expression text (`v-if="user.isActive"`,
/// `{user.name}`, etc.). Delegates to swc's expression parser and walks
/// `Ident` nodes filtering JS globals listed in [`JS_GLOBALS`].
///
/// Member-prop names (`name` in `user.name`) are NOT emitted because swc
/// represents them as `MemberProp::Ident` (an `IdentName`, not an
/// `Ident`) and the [`Visit`] trait's `visit_ident` only fires on the
/// expression-position `Ident`s — exactly the references we want.
///
/// Silently no-ops on parse failure: a broken expression in a partial
/// edit shouldn't abort the whole template walk.
pub(crate) fn extract_identifiers_from_expression(
    expr_src: &str,
    base_offset: usize,
    attribute: TemplateAttribute,
    sink: &mut dyn TemplateRefSink,
) {
    let trimmed = expr_src.trim();
    if trimmed.is_empty() {
        return;
    }
    let cm: Lrc<SourceMap> = Lrc::new(SourceMap::default());
    let fm = cm.new_source_file(Lrc::new(FileName::Anon), expr_src.to_string());
    let file_start = fm.start_pos.0;
    let lexer = Lexer::new(
        Syntax::Es(EsSyntax::default()),
        EsVersion::EsNext,
        StringInput::from(&*fm),
        None,
    );
    let mut parser = Parser::new_from(lexer);
    let Ok(expr) = parser.parse_expr() else {
        return;
    };
    let mut collector = IdentCollector {
        base_offset,
        attribute,
        sink,
        file_start,
    };
    expr.visit_with(&mut collector);
}

/// JS globals filtered out of template identifier extraction. Keeps the
/// graph free of noise from `Math.max(...)`, `JSON.stringify(...)`,
/// `console.log(...)`, etc. — none of these are project symbols.
/// Shared with the JSX visitor in [`crate::ts::visit`] (pivot — same
/// vocabulary, same noise, same filter).
pub(crate) const JS_GLOBALS: &[&str] = &[
    "console",
    "window",
    "document",
    "globalThis",
    "self",
    "undefined",
    "NaN",
    "Infinity",
    "Math",
    "Date",
    "Object",
    "Array",
    "Number",
    "String",
    "Boolean",
    "JSON",
    "RegExp",
    "Symbol",
    "Error",
    "TypeError",
    "RangeError",
    "Map",
    "Set",
    "WeakMap",
    "WeakSet",
    "Promise",
    "Proxy",
    "Reflect",
    "parseInt",
    "parseFloat",
    "isNaN",
    "isFinite",
    "encodeURI",
    "encodeURIComponent",
    "decodeURI",
    "decodeURIComponent",
];

struct IdentCollector<'a> {
    base_offset: usize,
    attribute: TemplateAttribute,
    sink: &'a mut dyn TemplateRefSink,
    /// `BytePos.0` of the file's first byte. swc's BytePos is 1-indexed
    /// across the whole `SourceMap` so `ident.span.lo.0 - file_start`
    /// is the byte offset within the parsed expression text.
    file_start: u32,
}

impl Visit for IdentCollector<'_> {
    fn visit_ident(&mut self, ident: &Ident) {
        let name = ident.sym.to_string();
        if JS_GLOBALS.contains(&name.as_str()) {
            return;
        }
        let local = (ident.span.lo.0.saturating_sub(self.file_start)) as usize;
        self.sink.push(TemplateRef {
            name,
            attribute: self.attribute,
            byte_offset: self.base_offset + local,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect(src: &str, attribute: TemplateAttribute) -> Vec<TemplateRef> {
        let mut sink: Vec<TemplateRef> = Vec::new();
        extract_identifiers_from_expression(src, 0, attribute, &mut sink);
        sink
    }

    #[test]
    fn simple_ident_is_emitted() {
        let refs = collect("user", TemplateAttribute::Bind);
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].name, "user");
        assert_eq!(refs[0].attribute, TemplateAttribute::Bind);
    }

    #[test]
    fn member_access_emits_only_the_object() {
        let refs = collect("user.name", TemplateAttribute::Interpolation);
        let names: Vec<&str> = refs.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["user"]);
    }

    #[test]
    fn nested_member_access_emits_only_root() {
        let refs = collect("user.profile.name", TemplateAttribute::Interpolation);
        let names: Vec<&str> = refs.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["user"]);
    }

    #[test]
    fn binary_op_emits_both_operands() {
        let refs = collect("a + b", TemplateAttribute::Interpolation);
        let names: Vec<&str> = refs.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["a", "b"]);
    }

    #[test]
    fn call_emits_callee_and_args() {
        let refs = collect("handleClick(payload)", TemplateAttribute::Event);
        let names: Vec<&str> = refs.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["handleClick", "payload"]);
    }

    #[test]
    fn js_globals_are_filtered() {
        let refs = collect("Math.max(a, b)", TemplateAttribute::Interpolation);
        let names: Vec<&str> = refs.iter().map(|r| r.name.as_str()).collect();
        assert!(!names.contains(&"Math"));
        assert!(names.contains(&"a"));
        assert!(names.contains(&"b"));
    }

    #[test]
    fn json_stringify_filtered() {
        let refs = collect("JSON.stringify(payload)", TemplateAttribute::Interpolation);
        let names: Vec<&str> = refs.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["payload"]);
    }

    #[test]
    fn console_log_filtered() {
        let refs = collect("console.log(msg)", TemplateAttribute::Event);
        let names: Vec<&str> = refs.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["msg"]);
    }

    #[test]
    fn empty_or_whitespace_input_is_noop() {
        assert!(collect("", TemplateAttribute::Bind).is_empty());
        assert!(collect("   ", TemplateAttribute::Bind).is_empty());
    }

    #[test]
    fn malformed_expression_is_silently_ignored() {
        let refs = collect("a +", TemplateAttribute::Bind);
        // Parser fails — no panic, no emit.
        assert!(refs.is_empty());
    }

    #[test]
    fn byte_offset_is_added_to_base() {
        let mut sink: Vec<TemplateRef> = Vec::new();
        extract_identifiers_from_expression("user", 100, TemplateAttribute::Bind, &mut sink);
        assert_eq!(sink[0].byte_offset, 100);
    }

    #[test]
    fn byte_offset_for_second_ident_in_binary_op() {
        let mut sink: Vec<TemplateRef> = Vec::new();
        // "a + b" → 'a' at 0, 'b' at 4 (after "a + ").
        extract_identifiers_from_expression(
            "a + b",
            10,
            TemplateAttribute::Interpolation,
            &mut sink,
        );
        let by_offset: std::collections::HashMap<&str, usize> = sink
            .iter()
            .map(|r| (r.name.as_str(), r.byte_offset))
            .collect();
        assert_eq!(by_offset["a"], 10);
        assert_eq!(by_offset["b"], 14);
    }

    #[test]
    fn shorthand_object_property_emits_value_ident() {
        // `{ user }` desugars to `{ user: user }` — the value is an Expr::Ident.
        let refs = collect("({ user })", TemplateAttribute::Bind);
        let names: Vec<&str> = refs.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["user"]);
    }

    #[test]
    fn ternary_emits_all_three_operand_idents() {
        let refs = collect(
            "flag ? primary : fallback",
            TemplateAttribute::Interpolation,
        );
        let names: Vec<&str> = refs.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["flag", "primary", "fallback"]);
    }

    #[test]
    fn unary_emits_inner() {
        let refs = collect("!disabled", TemplateAttribute::Bind);
        let names: Vec<&str> = refs.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["disabled"]);
    }

    #[test]
    fn arrow_param_is_not_filtered_day_1() {
        // Day-1 we don't introduce scope tracking. Arrow params show up as
        // refs alongside body refs — accepted noise in exchange for parser
        // simplicity (cohérent feedback_scope_graph_not_lsp).
        let refs = collect("e => handleClick(e)", TemplateAttribute::Event);
        let names: Vec<&str> = refs.iter().map(|r| r.name.as_str()).collect();
        assert!(names.contains(&"handleClick"));
        assert!(names.contains(&"e"));
    }

    #[test]
    fn template_attribute_carried_through() {
        let event = collect("handleClick", TemplateAttribute::Event);
        let bind = collect("handleClick", TemplateAttribute::Bind);
        let comp = collect("handleClick", TemplateAttribute::ComponentRef);
        assert_eq!(event[0].attribute, TemplateAttribute::Event);
        assert_eq!(bind[0].attribute, TemplateAttribute::Bind);
        assert_eq!(comp[0].attribute, TemplateAttribute::ComponentRef);
    }
}

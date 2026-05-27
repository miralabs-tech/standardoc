use standardoc_ir::{Blake3Hash, Kind, LanguageKind, Visibility};

use super::*;

fn sym(name: &str, fqdn: &str, kind: Kind, loc: SymbolLocation) -> RawSymbol {
    RawSymbol {
        decl_kind: None,
        implements_trait: None,
        receiver_type: None,
        entry_point: None,
        name: name.into(),
        fqdn: fqdn.into(),
        kind,
        language_kind: LanguageKind::from("fn_item"),
        module: None,
        visibility: Visibility::Public,
        location: loc,
        signature: None,
        body_hash: Some(Blake3Hash::default()),
        attributes: vec![],
        flags: vec![],
    }
}

fn loc(start_line: u32, end_line: u32, start_col: u32, end_col: u32) -> SymbolLocation {
    SymbolLocation {
        file: "src/main.rs".into(),
        start_line,
        end_line,
        start_col,
        end_col,
    }
}

#[test]
fn range_contains_strict_containment() {
    assert!(range_contains(&loc(1, 100, 0, 0), &loc(10, 20, 4, 1)));
    assert!(!range_contains(&loc(10, 20, 4, 1), &loc(1, 100, 0, 0)));
    assert!(range_contains(&loc(5, 5, 0, 100), &loc(5, 5, 10, 90)));
}

#[test]
fn nest_document_symbols_attaches_inner_to_outer() {
    let outer = sym("outer", "crate::outer", Kind::Module, loc(1, 100, 0, 0));
    let inner = sym(
        "inner",
        "crate::outer::inner",
        Kind::Callable,
        loc(10, 20, 4, 1),
    );
    let nested = nest_document_symbols(vec![outer, inner]);
    assert_eq!(nested.len(), 1);
    assert_eq!(nested[0].name, "outer");
    let kids = nested[0].children.as_ref().expect("inner is a child");
    assert_eq!(kids.len(), 1);
    assert_eq!(kids[0].name, "inner");
    assert!(kids[0].children.is_none());
}

#[test]
fn nest_document_symbols_keeps_siblings_at_root_when_disjoint() {
    let a = sym("a", "crate::a", Kind::Callable, loc(1, 5, 0, 1));
    let b = sym("b", "crate::b", Kind::Callable, loc(10, 15, 0, 1));
    let nested = nest_document_symbols(vec![a, b]);
    assert_eq!(nested.len(), 2);
    assert_eq!(nested[0].name, "a");
    assert_eq!(nested[1].name, "b");
}

#[test]
fn to_lsp_position_decrements_line() {
    assert_eq!(
        to_lsp_position(10, 4),
        Position {
            line: 9,
            character: 4
        }
    );
    assert_eq!(
        to_lsp_position(0, 0),
        Position {
            line: 0,
            character: 0
        }
    );
}

#[test]
fn kind_to_lsp_maps_each_variant() {
    assert_eq!(kind_to_lsp(Kind::Callable), SymbolKind::FUNCTION);
    assert_eq!(kind_to_lsp(Kind::Type), SymbolKind::CLASS);
    assert_eq!(kind_to_lsp(Kind::Value), SymbolKind::VARIABLE);
    assert_eq!(kind_to_lsp(Kind::Module), SymbolKind::MODULE);
    assert_eq!(kind_to_lsp(Kind::Macro), SymbolKind::OPERATOR);
}

#[test]
fn render_hover_markdown_includes_signature_and_descriptions() {
    let mut s = sym("foo", "crate::foo", Kind::Callable, loc(1, 5, 0, 1));
    s.signature = Some(Signature {
        params: vec![],
        returns: None,
        ..Default::default()
    });
    let ctx = SymbolContext {
        symbol: s,
        language: "rust".into(),
        enrichment_description: Some("auto".into()),
        document_description: Some("manual".into()),
    };
    let md = render_hover_markdown(&ctx);
    assert!(md.contains("```rust"));
    assert!(md.contains("crate::foo"));
    assert!(md.contains("manual"));
    assert!(md.contains("auto"));
    assert!(md.contains("---"));
}

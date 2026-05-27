
use super::super::walk::walk;
use standardoc_ir::{EdgeKind, ResolvedOrUnresolved};

fn parse(src: &str) -> syn::File {
    syn::parse_file(src).expect("test source not parsable")
}

fn imports(edges: &[standardoc_ir::RawEdge]) -> Vec<&standardoc_ir::RawEdge> {
    edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Imports)
        .collect()
}

#[test]
fn simple_use_emits_one_import_edge() {
    let parsed = parse("use std::collections::HashMap;");
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let imp = imports(&edges);
    assert_eq!(imp.len(), 1);
    assert_eq!(imp[0].from_fqdn, "c");
    match &imp[0].to {
        ResolvedOrUnresolved::Unresolved { name } => {
            assert_eq!(name, "std::collections::HashMap");
        }
        other => panic!("expected unresolved, got {other:?}"),
    }
}

#[test]
fn use_group_emits_one_import_per_leaf() {
    let parsed = parse("use foo::{a, b, c};");
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let imp = imports(&edges);
    assert_eq!(imp.len(), 3);
    let names: Vec<_> = imp
        .iter()
        .map(|e| match &e.to {
            ResolvedOrUnresolved::Unresolved { name } => name.clone(),
            other => panic!("expected unresolved, got {other:?}"),
        })
        .collect();
    assert!(names.contains(&"foo::a".to_string()));
    assert!(names.contains(&"foo::b".to_string()));
    assert!(names.contains(&"foo::c".to_string()));
}

#[test]
fn use_glob_emits_import_to_prefix() {
    let parsed = parse("use foo::*;");
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let imp = imports(&edges);
    assert_eq!(imp.len(), 1);
    match &imp[0].to {
        ResolvedOrUnresolved::Unresolved { name } => assert_eq!(name, "foo"),
        other => panic!("expected unresolved, got {other:?}"),
    }
}

#[test]
fn use_rename_populates_alias() {
    let parsed = parse("use foo::Bar as B; fn use_it() { B::new(); }");
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let imp = imports(&edges);
    assert_eq!(imp.len(), 1);
    match &imp[0].to {
        ResolvedOrUnresolved::Unresolved { name } => assert_eq!(name, "foo::Bar"),
        other => panic!("expected unresolved, got {other:?}"),
    }
    // The CALLS edge should resolve B::new through the alias.
    let calls: Vec<_> = edges.iter().filter(|e| e.kind == EdgeKind::Calls).collect();
    assert_eq!(calls.len(), 1);
    match &calls[0].to {
        ResolvedOrUnresolved::Unresolved { name } => assert_eq!(name, "foo::Bar::new"),
        other => panic!("expected unresolved canonical via alias, got {other:?}"),
    }
}

#[test]
fn use_crate_relative_canonicalizes_against_crate_name() {
    let parsed = parse("use crate::foo::bar;");
    let (_, edges, _, _) = walk(&parsed, "mycrate", "src/lib.rs", "mycrate");
    let imp = imports(&edges);
    assert_eq!(imp.len(), 1);
    match &imp[0].to {
        ResolvedOrUnresolved::Unresolved { name } => assert_eq!(name, "mycrate::foo::bar"),
        other => panic!("expected unresolved canonical, got {other:?}"),
    }
}

#[test]
fn use_self_relative_canonicalizes_against_current_module() {
    let parsed = parse("use self::sub::thing;");
    let (_, edges, _, _) = walk(&parsed, "mycrate::a", "src/a.rs", "mycrate");
    let imp = imports(&edges);
    assert_eq!(imp.len(), 1);
    match &imp[0].to {
        ResolvedOrUnresolved::Unresolved { name } => {
            assert_eq!(name, "mycrate::a::sub::thing");
        }
        other => panic!("expected unresolved canonical, got {other:?}"),
    }
}

#[test]
fn use_super_pops_one_module_level() {
    let parsed = parse("use super::sibling;");
    let (_, edges, _, _) = walk(&parsed, "mycrate::a::b", "src/a/b.rs", "mycrate");
    let imp = imports(&edges);
    assert_eq!(imp.len(), 1);
    match &imp[0].to {
        ResolvedOrUnresolved::Unresolved { name } => {
            assert_eq!(name, "mycrate::a::sibling");
        }
        other => panic!("expected unresolved canonical, got {other:?}"),
    }
}

#[test]
fn extern_crate_emits_import_and_alias() {
    let parsed = parse("extern crate alloc as a; fn use_it() { a::vec::Vec::new(); }");
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let imp = imports(&edges);
    assert_eq!(imp.len(), 1);
    match &imp[0].to {
        ResolvedOrUnresolved::Unresolved { name } => assert_eq!(name, "alloc"),
        other => panic!("expected unresolved, got {other:?}"),
    }
    // CALLS through the alias.
    let calls: Vec<_> = edges.iter().filter(|e| e.kind == EdgeKind::Calls).collect();
    assert_eq!(calls.len(), 1);
    match &calls[0].to {
        ResolvedOrUnresolved::Unresolved { name } => assert_eq!(name, "alloc::vec::Vec::new"),
        other => panic!("expected unresolved via alias, got {other:?}"),
    }
}

#[test]
fn import_resolved_when_target_defined_in_same_file() {
    let parsed = parse("pub mod foo { pub fn bar() {} } use crate::foo::bar;");
    let (_, edges, _, _) = walk(&parsed, "mycrate", "src/lib.rs", "mycrate");
    let imp = imports(&edges);
    assert_eq!(imp.len(), 1);
    match &imp[0].to {
        ResolvedOrUnresolved::Resolved { fqdn } => assert_eq!(fqdn, "mycrate::foo::bar"),
        other => panic!("expected resolved (defined locally), got {other:?}"),
    }
}

#[test]
fn nested_use_groups_emit_one_import_per_leaf() {
    let parsed = parse("use std::{io::{Read, Write}, fmt};");
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let imp = imports(&edges);
    let names: Vec<String> = imp
        .iter()
        .map(|e| match &e.to {
            ResolvedOrUnresolved::Unresolved { name } => name.clone(),
            _ => panic!("expected unresolved"),
        })
        .collect();
    assert_eq!(names.len(), 3);
    assert!(names.contains(&"std::io::Read".to_string()));
    assert!(names.contains(&"std::io::Write".to_string()));
    assert!(names.contains(&"std::fmt".to_string()));
}

#[test]
fn pub_use_emits_phantom_symbol_and_marks_edge_as_re_export() {
    let parsed = parse("pub use foo::Bar;");
    let (symbols, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");

    let phantom = symbols
        .iter()
        .find(|s| s.fqdn == "c::Bar")
        .expect("phantom re-export symbol must be emitted at the short fqdn");
    assert_eq!(phantom.name, "Bar");
    assert!(matches!(phantom.kind, standardoc_ir::Kind::Type));
    assert_eq!(phantom.language_kind.as_str(), "re_export");
    assert!(matches!(
        phantom.visibility,
        standardoc_ir::Visibility::Public
    ));

    let imp = imports(&edges);
    assert_eq!(imp.len(), 1);
    assert!(
        imp[0].attributes.contains(&"re-export".to_string()),
        "edge attributes must mark this as a re-export, got {:?}",
        imp[0].attributes
    );
}

#[test]
fn non_pub_use_does_not_emit_phantom_or_re_export_attribute() {
    let parsed = parse("use foo::Bar;");
    let (symbols, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");

    assert!(
        !symbols.iter().any(|s| s.fqdn == "c::Bar"),
        "non-pub `use` must not produce a phantom symbol"
    );
    let imp = imports(&edges);
    assert_eq!(imp.len(), 1);
    assert!(
        imp[0].attributes.is_empty(),
        "private use must not carry re-export attribute, got {:?}",
        imp[0].attributes
    );
}

#[test]
fn pub_use_with_alias_emits_phantom_at_alias_fqdn() {
    let parsed = parse("pub use foo::Bar as B;");
    let (symbols, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");

    assert!(
        symbols.iter().any(|s| s.fqdn == "c::B"),
        "phantom must use the alias name, not the original"
    );
    assert!(
        !symbols.iter().any(|s| s.fqdn == "c::Bar"),
        "original name must not leak when an alias is given"
    );
    let imp = imports(&edges);
    assert!(imp[0].attributes.contains(&"re-export".to_string()));
}

#[test]
fn pub_use_glob_emits_wildcard_re_export_edge_no_phantom() {
    let parsed = parse("pub use foo::*;");
    let (symbols, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");

    // No phantom symbol for wildcard re-exports — we cannot enumerate
    // the target module's items in a single-file pass.
    assert!(
        symbols.is_empty() || !symbols.iter().any(|s| s.fqdn.contains('*')),
        "wildcard re-export must not synthesize a phantom"
    );
    let imp = imports(&edges);
    assert_eq!(imp.len(), 1);
    assert!(imp[0].attributes.contains(&"re-export".to_string()));
    assert!(imp[0].attributes.contains(&"wildcard".to_string()));
}

#[test]
fn pub_use_group_emits_one_phantom_per_leaf() {
    let parsed = parse("pub use foo::{a, b};");
    let (symbols, _, _, _) = walk(&parsed, "c", "src/lib.rs", "c");

    assert!(symbols.iter().any(|s| s.fqdn == "c::a"));
    assert!(symbols.iter().any(|s| s.fqdn == "c::b"));
}

#[test]
fn use_self_in_group_imports_the_prefix_itself() {
    let parsed = parse("use foo::{self, bar};");
    let (_, edges, _, _) = walk(&parsed, "c", "src/lib.rs", "c");
    let imp = imports(&edges);
    let names: Vec<String> = imp
        .iter()
        .map(|e| match &e.to {
            ResolvedOrUnresolved::Unresolved { name } => name.clone(),
            _ => panic!("expected unresolved"),
        })
        .collect();
    assert_eq!(names.len(), 2);
    assert!(names.contains(&"foo".to_string()));
    assert!(names.contains(&"foo::bar".to_string()));
}

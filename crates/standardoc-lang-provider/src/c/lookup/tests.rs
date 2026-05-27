
use super::*;
use standardoc_ir::{
    EdgeConfidence, EdgeKind, LanguageKind, RawEdge, RawSymbol, Site, SymbolLocation, Visibility,
};

fn sym(name: &str, fqdn: &str, kind: Kind, lang_kind: &str, vis: Visibility) -> RawSymbol {
    RawSymbol {
        decl_kind: None,
        implements_trait: None,
        receiver_type: None,
        entry_point: None,
        name: name.into(),
        fqdn: fqdn.into(),
        kind,
        language_kind: LanguageKind::from(lang_kind),
        module: fqdn.rsplit_once("::").map(|(m, _)| m.to_string()),
        visibility: vis,
        location: SymbolLocation {
            file: "src/a.c".into(),
            start_line: 1,
            end_line: 1,
            start_col: 0,
            end_col: 1,
        },
        signature: None,
        body_hash: None,
        attributes: vec![],
        flags: vec![],
    }
}

fn module_sym(module_fqdn: &str) -> RawSymbol {
    let parent = module_fqdn.rsplit_once("::").map(|(m, _)| m.to_string());
    RawSymbol {
        decl_kind: None,
        implements_trait: None,
        receiver_type: None,
        entry_point: None,
        name: module_fqdn
            .rsplit("::")
            .next()
            .unwrap_or(module_fqdn)
            .to_string(),
        fqdn: module_fqdn.into(),
        kind: Kind::Module,
        language_kind: LanguageKind::from("module"),
        module: parent,
        visibility: Visibility::Public,
        location: SymbolLocation {
            file: "src/a.c".into(),
            start_line: 1,
            end_line: 1,
            start_col: 0,
            end_col: 1,
        },
        signature: None,
        body_hash: None,
        attributes: vec![],
        flags: vec![],
    }
}

fn imports_edge(from: &str, target: ResolvedOrUnresolved) -> RawEdge {
    RawEdge {
        from_fqdn: from.into(),
        kind: EdgeKind::Imports,
        to: target,
        sites: vec![Site {
            file: "src/a.c".into(),
            line: 1,
            col: 0,
        }],
        attributes: vec![],
        confidence: EdgeConfidence::Extracted,
    }
}

#[test]
fn top_level_public_fn_emits_root_binding_with_resolved_fqdn() {
    let symbols = vec![
        module_sym("pkg::a"),
        sym(
            "do_work",
            "pkg::a::do_work",
            Kind::Callable,
            "fn",
            Visibility::Public,
        ),
    ];
    let lookup = build_c_lookup(&symbols, &[], "pkg::a");
    let entries = lookup.bindings.get("do_work").expect("root binding");
    assert_eq!(entries.len(), 1);
    let entry = &entries[0];
    assert_eq!(entry.scope_idx, ModuleLookup::ROOT_SCOPE);
    assert_eq!(entry.resolved_fqdn.as_deref(), Some("pkg::a::do_work"));
    assert!(matches!(
        entry.source,
        BindingSource::LocalDecl {
            decl_kind: LocalDeclKind::Function,
        }
    ));
}

#[test]
fn static_fn_is_excluded_from_bindings() {
    let symbols = vec![
        module_sym("pkg::a"),
        sym(
            "internal",
            "pkg::a::internal",
            Kind::Callable,
            "fn",
            Visibility::Private,
        ),
    ];
    let lookup = build_c_lookup(&symbols, &[], "pkg::a");
    assert!(!lookup.bindings.contains_key("internal"));
}

#[test]
fn struct_typedef_and_enum_emit_typed_decl_kinds() {
    let symbols = vec![
        module_sym("pkg::a"),
        sym(
            "Point",
            "pkg::a::Point",
            Kind::Type,
            "struct",
            Visibility::Public,
        ),
        sym(
            "u32",
            "pkg::a::u32",
            Kind::Type,
            "typedef",
            Visibility::Public,
        ),
        sym(
            "Color",
            "pkg::a::Color",
            Kind::Type,
            "enum",
            Visibility::Public,
        ),
    ];
    let lookup = build_c_lookup(&symbols, &[], "pkg::a");
    for (name, expected) in [
        ("Point", LocalDeclKind::Struct),
        ("u32", LocalDeclKind::TypeAlias),
        ("Color", LocalDeclKind::Enum),
    ] {
        let entry = &lookup.bindings.get(name).expect("binding")[0];
        let BindingSource::LocalDecl { decl_kind } = &entry.source else {
            panic!("expected LocalDecl, got {:?}", entry.source);
        };
        assert_eq!(decl_kind, &expected, "{name}");
    }
}

#[test]
fn sub_symbols_under_parent_type_are_excluded() {
    // Struct field with module pointing at the parent type — not
    // file-scoped, so it must NOT appear in bindings.
    let symbols = vec![
        module_sym("pkg::a"),
        sym(
            "Point",
            "pkg::a::Point",
            Kind::Type,
            "struct",
            Visibility::Public,
        ),
        sym(
            "x",
            "pkg::a::Point::x",
            Kind::Value,
            "field",
            Visibility::Public,
        ),
    ];
    let lookup = build_c_lookup(&symbols, &[], "pkg::a");
    assert!(lookup.bindings.contains_key("Point"));
    assert!(!lookup.bindings.contains_key("x"));
}

#[test]
fn module_symbol_itself_is_not_pushed_as_binding() {
    let symbols = vec![module_sym("pkg::a")];
    let lookup = build_c_lookup(&symbols, &[], "pkg::a");
    assert!(lookup.bindings.is_empty());
}

#[test]
fn system_include_emits_import_record_with_builtin_origin() {
    let symbols = vec![module_sym("pkg::a")];
    let edges = vec![imports_edge(
        "pkg::a",
        ResolvedOrUnresolved::Resolved {
            fqdn: "<builtin>::c::stdio".into(),
        },
    )];
    let lookup = build_c_lookup(&symbols, &edges, "pkg::a");
    assert_eq!(lookup.imports.len(), 1);
    let record = &lookup.imports[0];
    assert_eq!(record.local_name, "stdio");
    assert_eq!(record.origin_module, "<builtin>::c::stdio");
    assert!(!record.is_type_only);
}

#[test]
fn local_include_emits_import_record_with_basename_local_name() {
    let symbols = vec![module_sym("pkg::a")];
    let edges = vec![imports_edge(
        "pkg::a",
        ResolvedOrUnresolved::Unresolved {
            name: "runtime/util.h".into(),
        },
    )];
    let lookup = build_c_lookup(&symbols, &edges, "pkg::a");
    assert_eq!(lookup.imports.len(), 1);
    assert_eq!(lookup.imports[0].local_name, "util");
    assert_eq!(lookup.imports[0].origin_module, "runtime/util.h");
}

#[test]
fn module_fqdn_and_language_are_set_on_the_lookup() {
    let symbols = vec![module_sym("pkg::a")];
    let lookup = build_c_lookup(&symbols, &[], "pkg::a");
    assert_eq!(lookup.module_fqdn, "pkg::a");
    assert_eq!(lookup.language, Language::C);
}

#[test]
fn non_imports_edge_is_skipped_when_building_import_records() {
    let symbols = vec![module_sym("pkg::a")];
    let edges = vec![RawEdge {
        from_fqdn: "pkg::a".into(),
        kind: EdgeKind::Calls,
        to: ResolvedOrUnresolved::Resolved {
            fqdn: "pkg::b::foo".into(),
        },
        sites: vec![],
        attributes: vec![],
        confidence: EdgeConfidence::Extracted,
    }];
    let lookup = build_c_lookup(&symbols, &edges, "pkg::a");
    assert!(lookup.imports.is_empty());
}

#[test]
fn unknown_type_language_kind_falls_back_to_custom() {
    let symbols = vec![
        module_sym("pkg::a"),
        sym("U", "pkg::a::U", Kind::Type, "union", Visibility::Public),
    ];
    let lookup = build_c_lookup(&symbols, &[], "pkg::a");
    let entry = &lookup.bindings.get("U").expect("binding")[0];
    let BindingSource::LocalDecl { decl_kind } = &entry.source else {
        panic!("expected LocalDecl");
    };
    assert!(matches!(
        decl_kind,
        LocalDeclKind::Custom { lang: Language::C, tag } if tag == "union"
    ));
}

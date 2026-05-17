use standardoc_core::ExtractError;
use standardoc_ir::{
    ExtractedFile, Kind, Language, LanguageKind, RawDocument, RawSymbol, SourceOrigin, Visibility,
};

use super::{extract_doc, module_path, walk};
use crate::utils::{file_span, hash_bytes, last_segment, parent_module};

pub(crate) fn extract_file(
    content: &str,
    path: &str,
    crate_name: &str,
    crate_rel: &str,
) -> Result<ExtractedFile, ExtractError> {
    let parsed = syn::parse_file(content).map_err(|e| ExtractError::Parse {
        file: path.into(),
        detail: e.to_string(),
    })?;

    let module_fqdn = module_path::compute(crate_name, crate_rel);
    let name = last_segment(&module_fqdn).to_string();
    let parent = parent_module(&module_fqdn);

    let content_hash = hash_bytes(content.as_bytes());

    let module_symbol = RawSymbol {
        name,
        fqdn: module_fqdn.clone(),
        kind: Kind::Module,
        language_kind: LanguageKind::from("module"),
        module: parent,
        visibility: Visibility::Public,
        location: file_span(path, content),
        signature: None,
        body_hash: Some(content_hash),
        attributes: vec![],
        flags: vec![],
    };

    let mut documents = Vec::new();
    if let Some(description) = extract_doc::extract_inner(&parsed.attrs) {
        documents.push(RawDocument {
            symbol_fqdn: module_fqdn.clone(),
            description,
        });
    }

    let mut symbols = vec![module_symbol];
    let (item_symbols, edges, item_documents) = walk::walk(&parsed, &module_fqdn, path, crate_name);
    symbols.extend(item_symbols);
    documents.extend(item_documents);

    Ok(ExtractedFile {
        file: path.into(),
        language: Language::Rust,
        source_origin: SourceOrigin::Workspace,
        is_external: false,
        content_hash,
        byte_size: u64::try_from(content.len()).unwrap_or(u64::MAX),
        symbols,
        edges,
        call_sites: vec![],
        documents,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_file_produces_module_symbol_only() {
        let r = extract_file("", "src/lib.rs", "foo", "src/lib.rs").unwrap();
        assert_eq!(r.symbols.len(), 1);
        assert_eq!(r.edges.len(), 0);
        assert_eq!(r.call_sites.len(), 0);
        assert_eq!(r.symbols[0].kind, Kind::Module);
    }

    #[test]
    fn syntax_error_returns_parse_error() {
        let err = extract_file("fn foo( {", "src/lib.rs", "foo", "src/lib.rs").unwrap_err();
        assert!(matches!(err, ExtractError::Parse { .. }));
    }

    #[test]
    fn lib_rs_uses_crate_name_as_module_name() {
        let r = extract_file("", "src/lib.rs", "mycrate", "src/lib.rs").unwrap();
        assert_eq!(r.symbols[0].fqdn, "mycrate");
        assert_eq!(r.symbols[0].name, "mycrate");
        assert_eq!(r.symbols[0].module, None);
    }

    #[test]
    fn foo_rs_module_name_and_parent() {
        let r = extract_file("", "src/foo.rs", "mycrate", "src/foo.rs").unwrap();
        assert_eq!(r.symbols[0].fqdn, "mycrate::foo");
        assert_eq!(r.symbols[0].name, "foo");
        assert_eq!(r.symbols[0].module.as_deref(), Some("mycrate"));
    }

    #[test]
    fn nested_module_path() {
        let r = extract_file("", "src/foo/bar/baz.rs", "mycrate", "src/foo/bar/baz.rs").unwrap();
        assert_eq!(r.symbols[0].fqdn, "mycrate::foo::bar::baz");
        assert_eq!(r.symbols[0].name, "baz");
        assert_eq!(r.symbols[0].module.as_deref(), Some("mycrate::foo::bar"));
    }

    #[test]
    fn mod_rs_collapses_to_dir_module() {
        let r = extract_file("", "src/foo/mod.rs", "mycrate", "src/foo/mod.rs").unwrap();
        assert_eq!(r.symbols[0].fqdn, "mycrate::foo");
        assert_eq!(r.symbols[0].name, "foo");
        assert_eq!(r.symbols[0].module.as_deref(), Some("mycrate"));
    }

    #[test]
    fn content_hash_equals_blake3_of_bytes() {
        use standardoc_ir::Blake3Hash;
        let content = "fn main() {}\n";
        let r = extract_file(content, "src/main.rs", "foo", "src/main.rs").unwrap();
        let expected = Blake3Hash::new(*blake3::hash(content.as_bytes()).as_bytes());
        assert_eq!(r.content_hash, expected);
    }

    #[test]
    fn module_body_hash_equals_content_hash() {
        let r = extract_file("// hi\n", "src/lib.rs", "foo", "src/lib.rs").unwrap();
        assert_eq!(r.symbols[0].body_hash, Some(r.content_hash));
    }

    #[test]
    fn byte_size_matches_content_len() {
        let content = "fn main() {}\n";
        let r = extract_file(content, "src/main.rs", "foo", "src/main.rs").unwrap();
        assert_eq!(r.byte_size, u64::try_from(content.len()).unwrap());
    }

    #[test]
    fn module_visibility_is_public() {
        let r = extract_file("", "src/lib.rs", "foo", "src/lib.rs").unwrap();
        assert_eq!(r.symbols[0].visibility, Visibility::Public);
    }

    #[test]
    fn module_location_covers_whole_file() {
        let content = "// line 1\n// line 2\n// line 3";
        let r = extract_file(content, "src/lib.rs", "foo", "src/lib.rs").unwrap();
        let loc = &r.symbols[0].location;
        assert_eq!(loc.start_line, 1);
        assert_eq!(loc.end_line, 3);
        assert_eq!(loc.start_col, 0);
        assert_eq!(loc.end_col, 9);
    }

    // `content_extent` itself moved to `crate::utils::location` and is
    // covered by its own unit tests there. The integration test
    // `module_location_covers_whole_file` above already validates that
    // `extract_file` consumes it correctly.

    #[test]
    fn module_language_is_rust() {
        let r = extract_file("", "src/lib.rs", "foo", "src/lib.rs").unwrap();
        assert_eq!(r.language, Language::Rust);
        assert_eq!(r.symbols[0].language_kind.as_str(), "module");
    }

    #[test]
    fn monorepo_crate_rel_strips_workspace_path_leak() {
        // Simulates the case where the user opens VSCode on a parent of
        // the actual Rust crate (e.g. `stdoc/` rather than
        // `stdoc/standardoc/`). The workspace-relative `path` carries
        // a leading `standardoc/crates/<crate>/...` segment, but the
        // `crate_rel` argument is correctly stripped to just `src/foo.rs`.
        // The resulting FQDN must NOT leak the workspace path.
        let r = extract_file(
            "",
            "standardoc/crates/standardoc-cli/src/foo.rs",
            "standardoc-cli",
            "src/foo.rs",
        )
        .unwrap();
        assert_eq!(r.symbols[0].fqdn, "standardoc-cli::foo");
        assert!(!r.symbols[0].fqdn.contains("standardoc::crates"));
        // Location still references the workspace-relative path.
        assert_eq!(
            r.symbols[0].location.file,
            "standardoc/crates/standardoc-cli/src/foo.rs"
        );
    }
}

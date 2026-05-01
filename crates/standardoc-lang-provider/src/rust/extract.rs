use standardoc_core::ExtractError;
use standardoc_ir::{
    Blake3Hash, ExtractedFile, Kind, Language, LanguageKind, RawSymbol, SourceOrigin,
    SymbolLocation, Visibility,
};

use super::{module_path, walk};

pub(crate) fn extract_file(
    content: &str,
    path: &str,
    crate_name: &str,
) -> Result<ExtractedFile, ExtractError> {
    let parsed = syn::parse_file(content).map_err(|e| ExtractError::Parse {
        file: path.into(),
        detail: e.to_string(),
    })?;

    let module_fqdn = module_path::compute(crate_name, path);
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
    };

    let mut symbols = vec![module_symbol];
    let (item_symbols, edges) = walk::walk(&parsed, &module_fqdn, path, crate_name);
    symbols.extend(item_symbols);

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
    })
}

fn hash_bytes(bytes: &[u8]) -> Blake3Hash {
    let digest = blake3::hash(bytes);
    Blake3Hash::new(*digest.as_bytes())
}

fn last_segment(fqdn: &str) -> &str {
    fqdn.rsplit("::").next().unwrap_or(fqdn)
}

fn parent_module(fqdn: &str) -> Option<String> {
    fqdn.rsplit_once("::").map(|(parent, _)| parent.to_string())
}

fn file_span(path: &str, content: &str) -> SymbolLocation {
    let (end_line, end_col) = content_extent(content);
    SymbolLocation {
        file: path.into(),
        start_line: 1,
        end_line,
        start_col: 0,
        end_col,
    }
}

fn content_extent(content: &str) -> (u32, u32) {
    if content.is_empty() {
        return (1, 0);
    }
    let line_count = u32::try_from(content.lines().count()).unwrap_or(u32::MAX);
    let last_col = content
        .lines()
        .last()
        .map_or(0, |l| u32::try_from(l.len()).unwrap_or(u32::MAX));
    (line_count, last_col)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_file_produces_module_symbol_only() {
        let r = extract_file("", "src/lib.rs", "foo").unwrap();
        assert_eq!(r.symbols.len(), 1);
        assert_eq!(r.edges.len(), 0);
        assert_eq!(r.call_sites.len(), 0);
        assert_eq!(r.symbols[0].kind, Kind::Module);
    }

    #[test]
    fn syntax_error_returns_parse_error() {
        let err = extract_file("fn foo( {", "src/lib.rs", "foo").unwrap_err();
        assert!(matches!(err, ExtractError::Parse { .. }));
    }

    #[test]
    fn lib_rs_uses_crate_name_as_module_name() {
        let r = extract_file("", "src/lib.rs", "mycrate").unwrap();
        assert_eq!(r.symbols[0].fqdn, "mycrate");
        assert_eq!(r.symbols[0].name, "mycrate");
        assert_eq!(r.symbols[0].module, None);
    }

    #[test]
    fn foo_rs_module_name_and_parent() {
        let r = extract_file("", "src/foo.rs", "mycrate").unwrap();
        assert_eq!(r.symbols[0].fqdn, "mycrate::foo");
        assert_eq!(r.symbols[0].name, "foo");
        assert_eq!(r.symbols[0].module.as_deref(), Some("mycrate"));
    }

    #[test]
    fn nested_module_path() {
        let r = extract_file("", "src/foo/bar/baz.rs", "mycrate").unwrap();
        assert_eq!(r.symbols[0].fqdn, "mycrate::foo::bar::baz");
        assert_eq!(r.symbols[0].name, "baz");
        assert_eq!(r.symbols[0].module.as_deref(), Some("mycrate::foo::bar"));
    }

    #[test]
    fn mod_rs_collapses_to_dir_module() {
        let r = extract_file("", "src/foo/mod.rs", "mycrate").unwrap();
        assert_eq!(r.symbols[0].fqdn, "mycrate::foo");
        assert_eq!(r.symbols[0].name, "foo");
        assert_eq!(r.symbols[0].module.as_deref(), Some("mycrate"));
    }

    #[test]
    fn content_hash_equals_blake3_of_bytes() {
        let content = "fn main() {}\n";
        let r = extract_file(content, "src/main.rs", "foo").unwrap();
        let expected = Blake3Hash::new(*blake3::hash(content.as_bytes()).as_bytes());
        assert_eq!(r.content_hash, expected);
    }

    #[test]
    fn module_body_hash_equals_content_hash() {
        let r = extract_file("// hi\n", "src/lib.rs", "foo").unwrap();
        assert_eq!(r.symbols[0].body_hash, Some(r.content_hash));
    }

    #[test]
    fn byte_size_matches_content_len() {
        let content = "fn main() {}\n";
        let r = extract_file(content, "src/main.rs", "foo").unwrap();
        assert_eq!(
            r.byte_size,
            u64::try_from(content.len()).unwrap()
        );
    }

    #[test]
    fn module_visibility_is_public() {
        let r = extract_file("", "src/lib.rs", "foo").unwrap();
        assert_eq!(r.symbols[0].visibility, Visibility::Public);
    }

    #[test]
    fn module_location_covers_whole_file() {
        let content = "// line 1\n// line 2\n// line 3";
        let r = extract_file(content, "src/lib.rs", "foo").unwrap();
        let loc = &r.symbols[0].location;
        assert_eq!(loc.start_line, 1);
        assert_eq!(loc.end_line, 3);
        assert_eq!(loc.start_col, 0);
        assert_eq!(loc.end_col, 9);
    }

    #[test]
    fn empty_content_extent_is_one_line_zero_col() {
        assert_eq!(content_extent(""), (1, 0));
    }

    #[test]
    fn single_line_no_newline_extent() {
        assert_eq!(content_extent("hello"), (1, 5));
    }

    #[test]
    fn trailing_newline_keeps_count_consistent() {
        assert_eq!(content_extent("a\nb\n"), (2, 1));
    }

    #[test]
    fn module_language_is_rust() {
        let r = extract_file("", "src/lib.rs", "foo").unwrap();
        assert_eq!(r.language, Language::Rust);
        assert_eq!(r.symbols[0].language_kind.as_str(), "module");
    }
}

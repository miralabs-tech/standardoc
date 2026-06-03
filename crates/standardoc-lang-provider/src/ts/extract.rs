use std::path::Path;

use standardoc_core::ExtractError;
use standardoc_ir::{
    DeclKind, ExtractedFile, Kind, Language, LanguageKind, RawDocument, RawSymbol, SourceOrigin,
    Visibility,
};
use swc_core::common::comments::SingleThreadedComments;
use swc_core::common::{FileName, SourceMap, Spanned, sync::Lrc};
use swc_core::ecma::ast::EsVersion;
use swc_core::ecma::parser::{EsSyntax, Parser, StringInput, Syntax, TsSyntax, lexer::Lexer};

use super::extract_doc;
use super::helpers::compute_module_path;
use super::resolver::TsConfigPaths;
use super::walk;
use crate::utils::{file_span, hash_bytes, last_segment, parent_module};

/// Lock 41 §2.5 wrapper. Lets the SFC orchestrator (Vue / Svelte) drive
/// the syntax + IR language explicitly while still routing through the
/// shared TS extractor.
///
/// `syntax_override` forces the swc lexer's syntax (used by the SFC
/// orchestrator to parse a `.vue`/`.svelte` `<script lang="ts">` body
/// as TS rather than guessing from the `.vue` extension).
///
/// `language_override` substitutes the IR `Language` tag stamped on the
/// resulting `ExtractedFile` (so a `.vue` file lands as `Language::Vue`
/// in the DB even though its symbols came out of the TS extractor).
///
/// The original `workspace_relative_path` is preserved verbatim — symbols
/// and edge sites still report the user-facing `.vue` / `.svelte` path.
#[allow(clippy::too_many_arguments)]
pub(crate) fn extract_file_with_syntax(
    content: &str,
    workspace_relative_path: &str,
    package_name: &str,
    package_relative: &str,
    from_file_abs_path: &Path,
    package_root: &Path,
    tsconfig: Option<TsConfigPaths>,
    syntax_override: Option<Syntax>,
    language_override: Option<Language>,
) -> Result<ExtractedFile, ExtractError> {
    extract_file_inner(
        content,
        workspace_relative_path,
        package_name,
        package_relative,
        from_file_abs_path,
        package_root,
        tsconfig,
        syntax_override,
        language_override,
    )
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn extract_file(
    content: &str,
    workspace_relative_path: &str,
    package_name: &str,
    package_relative: &str,
    from_file_abs_path: &Path,
    package_root: &Path,
    tsconfig: Option<TsConfigPaths>,
) -> Result<ExtractedFile, ExtractError> {
    extract_file_inner(
        content,
        workspace_relative_path,
        package_name,
        package_relative,
        from_file_abs_path,
        package_root,
        tsconfig,
        None,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
fn extract_file_inner(
    content: &str,
    workspace_relative_path: &str,
    package_name: &str,
    package_relative: &str,
    from_file_abs_path: &Path,
    package_root: &Path,
    tsconfig: Option<TsConfigPaths>,
    syntax_override: Option<Syntax>,
    language_override: Option<Language>,
) -> Result<ExtractedFile, ExtractError> {
    let cm: Lrc<SourceMap> = Default::default();
    let fm = cm.new_source_file(
        Lrc::new(FileName::Custom(workspace_relative_path.into())),
        content.to_string(),
    );
    let comments = SingleThreadedComments::default();
    let syntax = syntax_override.unwrap_or_else(|| syntax_for(workspace_relative_path));
    let lexer = Lexer::new(
        syntax,
        EsVersion::EsNext,
        StringInput::from(&*fm),
        Some(&comments),
    );
    let mut parser = Parser::new_from(lexer);
    let module = parser.parse_module().map_err(|e| ExtractError::Parse {
        file: workspace_relative_path.into(),
        detail: format!("{e:?}"),
    })?;

    let module_path = compute_module_path(package_relative);
    let module_fqdn = if module_path.is_empty() {
        package_name.to_string()
    } else {
        format!("{package_name}::{module_path}")
    };

    let content_hash = hash_bytes(content.as_bytes());
    let module_symbol = RawSymbol {
        decl_kind: Some(DeclKind::Module),
        implements_trait: None,
        receiver_type: None,
        entry_point: None,
        name: last_segment(&module_fqdn).to_string(),
        fqdn: module_fqdn.clone(),
        kind: Kind::Module,
        language_kind: LanguageKind::from("module"),
        module: parent_module(&module_fqdn),
        visibility: Visibility::Public,
        location: file_span(workspace_relative_path, content),
        signature: None,
        body_hash: Some(content_hash),
        attributes: vec![],
        flags: vec![],
    };

    let mut documents = Vec::new();
    if let Some(first_pos) = module.body.first().map(|item| item.span().lo)
        && let Some(description) = extract_doc::extract_at(&comments, first_pos)
    {
        documents.push(RawDocument {
            symbol_fqdn: module_fqdn.clone(),
            description,
        });
    }

    let mut symbols = vec![module_symbol];
    let (ffi_symbols, ffi_bindings) = super::ffi_tagger::extract_ffi_bindings(
        &module,
        &module_fqdn,
        workspace_relative_path,
        &cm,
    );
    symbols.extend(ffi_symbols);
    let (item_symbols, edges, item_documents, call_sites, lookup) = walk::walk_with_lookup(
        &module,
        package_name,
        workspace_relative_path,
        &module_fqdn,
        cm,
        from_file_abs_path,
        package_root,
        tsconfig,
        &comments,
    );
    symbols.extend(item_symbols);
    documents.extend(item_documents);

    Ok(ExtractedFile {
        file: workspace_relative_path.into(),
        language: language_override.unwrap_or_else(|| language_for(workspace_relative_path)),
        source_origin: SourceOrigin::Workspace,
        is_external: false,
        content_hash,
        byte_size: u64::try_from(content.len()).unwrap_or(u64::MAX),
        symbols,
        edges,
        call_sites,
        documents,
        ffi_bindings,
        module_lookup: Some(lookup),
    })
}

fn syntax_for(path: &str) -> Syntax {
    let ext = Path::new(path)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    match ext {
        "tsx" => Syntax::Typescript(TsSyntax {
            tsx: true,
            decorators: false,
            dts: path.ends_with(".d.tsx"),
            no_early_errors: true,
            disallow_ambiguous_jsx_like: false,
        }),
        "ts" | "mts" | "cts" => Syntax::Typescript(TsSyntax {
            tsx: false,
            decorators: false,
            dts: path.ends_with(".d.ts") || path.ends_with(".d.mts") || path.ends_with(".d.cts"),
            no_early_errors: true,
            disallow_ambiguous_jsx_like: false,
        }),
        "jsx" => Syntax::Es(EsSyntax {
            jsx: true,
            ..Default::default()
        }),
        _ => Syntax::Es(EsSyntax::default()),
    }
}

fn language_for(path: &str) -> Language {
    let ext = Path::new(path)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    match ext {
        "ts" | "tsx" | "mts" | "cts" => Language::TypeScript,
        _ => Language::JavaScript,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn extract(content: &str, workspace_relative: &str, package_relative: &str) -> ExtractedFile {
        extract_file(
            content,
            workspace_relative,
            "@app",
            package_relative,
            &PathBuf::from(format!("/tmp/pkg/{package_relative}")),
            &PathBuf::from("/tmp/pkg"),
            None,
        )
        .expect("extract ok")
    }

    #[test]
    fn empty_file_produces_module_symbol_only() {
        let r = extract("", "src/index.ts", "src/index.ts");
        assert_eq!(r.symbols.len(), 1);
        assert!(r.edges.is_empty());
        assert_eq!(r.symbols[0].kind, Kind::Module);
    }

    #[test]
    fn syntax_error_returns_parse_error() {
        let err = extract_file(
            "function foo( {",
            "src/index.ts",
            "@app",
            "src/index.ts",
            &PathBuf::from("/tmp/pkg/src/index.ts"),
            &PathBuf::from("/tmp/pkg"),
            None,
        )
        .expect_err("syntax error");
        assert!(matches!(err, ExtractError::Parse { .. }));
    }

    #[test]
    fn module_fqdn_uses_package_name_for_index() {
        let r = extract("", "src/index.ts", "src/index.ts");
        assert_eq!(r.symbols[0].fqdn, "@app::src");
    }

    #[test]
    fn module_fqdn_for_nested_file() {
        let r = extract("", "src/auth/login.ts", "src/auth/login.ts");
        assert_eq!(r.symbols[0].fqdn, "@app::src::auth::login");
    }

    #[test]
    fn module_fqdn_for_top_level_index_collapses_to_package_name() {
        let r = extract("", "index.ts", "index.ts");
        assert_eq!(r.symbols[0].fqdn, "@app");
    }

    #[test]
    fn content_hash_equals_blake3_of_bytes() {
        use standardoc_ir::Blake3Hash;
        let content = "export function foo() {}\n";
        let r = extract(content, "src/foo.ts", "src/foo.ts");
        let expected = Blake3Hash::new(*blake3::hash(content.as_bytes()).as_bytes());
        assert_eq!(r.content_hash, expected);
    }

    #[test]
    fn module_body_hash_equals_content_hash() {
        let r = extract("// hi\n", "src/index.ts", "src/index.ts");
        assert_eq!(r.symbols[0].body_hash, Some(r.content_hash));
    }

    #[test]
    fn byte_size_matches_content_len() {
        let content = "export const N = 1;\n";
        let r = extract(content, "src/n.ts", "src/n.ts");
        assert_eq!(r.byte_size, u64::try_from(content.len()).unwrap());
    }

    #[test]
    fn ts_extension_maps_to_typescript_language() {
        let r = extract("", "src/index.ts", "src/index.ts");
        assert_eq!(r.language, Language::TypeScript);
    }

    #[test]
    fn js_extension_maps_to_javascript_language() {
        let r = extract("", "src/index.js", "src/index.js");
        assert_eq!(r.language, Language::JavaScript);
    }

    #[test]
    fn tsx_extension_maps_to_typescript_with_jsx_supported() {
        let r = extract(
            "export const App = () => <div>Hi</div>;\n",
            "src/App.tsx",
            "src/App.tsx",
        );
        assert_eq!(r.language, Language::TypeScript);
    }

    #[test]
    fn jsx_extension_maps_to_javascript_with_jsx_supported() {
        let r = extract(
            "export const App = () => <div>Hi</div>;\n",
            "src/App.jsx",
            "src/App.jsx",
        );
        assert_eq!(r.language, Language::JavaScript);
    }

    #[test]
    fn module_symbol_visibility_is_public() {
        let r = extract("", "src/index.ts", "src/index.ts");
        assert_eq!(r.symbols[0].visibility, Visibility::Public);
    }

    #[test]
    fn module_location_covers_whole_file() {
        let content = "// line 1\n// line 2\n// line 3";
        let r = extract(content, "src/index.ts", "src/index.ts");
        let loc = &r.symbols[0].location;
        assert_eq!(loc.start_line, 1);
        assert_eq!(loc.end_line, 3);
        assert_eq!(loc.start_col, 0);
        assert_eq!(loc.end_col, 9);
    }

    // `content_extent` moved to `crate::utils::location` and is covered
    // by its own unit tests there.

    #[test]
    fn realistic_file_extracts_module_plus_items() {
        let src = "
            export interface User { id: string; }
            export function makeUser(id: string): User { return { id }; }
            export class UserService {
              create(id: string): User { return makeUser(id); }
            }
        ";
        let r = extract(src, "src/user/service.ts", "src/user/service.ts");
        let names: Vec<&str> = r.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"service"));
        assert!(names.contains(&"User"));
        assert!(names.contains(&"makeUser"));
        assert!(names.contains(&"UserService"));
        assert!(names.contains(&"create"));
        let calls = r
            .edges
            .iter()
            .filter(|e| e.kind == standardoc_ir::EdgeKind::Calls)
            .count();
        assert!(calls >= 1, "expected at least one CALLS edge");
    }
}

use standardoc_core::ExtractError;
use standardoc_ir::{
    ExtractedFile, Kind, Language, LanguageKind, RawSymbol, SourceOrigin, Visibility,
};
use tree_sitter::Parser;

use crate::utils::{file_span, hash_bytes, last_segment, parent_module};

use super::helpers::compute_module_path;
use super::walk::{CWalkContext, walk_translation_unit};

/// Parse a C source file with `tree-sitter-c`, walk it, and return an
/// `ExtractedFile` ready for the pipeline. MVP scope: emits fn defs +
/// fn decls + structs/unions/enums + typedefs + macros. No cross-file
/// join (Stage 1c), no preprocessor expansion, no FFI tagging (Stage 2).
pub(crate) fn extract_file(
    content: &str,
    workspace_relative_path: &str,
    package_name: &str,
    package_relative: &str,
) -> Result<ExtractedFile, ExtractError> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_c::LANGUAGE.into())
        .map_err(|e| ExtractError::Parse {
            file: workspace_relative_path.into(),
            detail: format!("load tree-sitter-c: {e}"),
        })?;

    let tree = parser.parse(content, None).ok_or_else(|| ExtractError::Parse {
        file: workspace_relative_path.into(),
        detail: "tree-sitter returned no parse tree".into(),
    })?;

    let module_path = compute_module_path(package_relative);
    let module_fqdn = if module_path.is_empty() {
        package_name.to_string()
    } else {
        format!("{package_name}::{module_path}")
    };

    let content_hash = hash_bytes(content.as_bytes());

    let module_symbol = RawSymbol {
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

    let mut ctx = CWalkContext::new(workspace_relative_path.to_string(), module_fqdn);
    ctx.core.push_symbol(module_symbol);

    walk_translation_unit(tree.root_node(), content, &mut ctx);

    Ok(ExtractedFile {
        file: workspace_relative_path.into(),
        language: Language::C,
        source_origin: SourceOrigin::Workspace,
        is_external: false,
        content_hash,
        byte_size: u64::try_from(content.len()).unwrap_or(u64::MAX),
        symbols: ctx.core.symbols,
        edges: ctx.core.edges,
        call_sites: ctx.core.call_sites,
        documents: ctx.core.documents,
    })
}

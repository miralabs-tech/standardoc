use standardoc_core::{ExtractContext, ExtractError, LanguageProvider};
use standardoc_ir::ExtractedFile;

mod extract;
mod helpers;
mod walk;

/// Native C `LanguageProvider` (tree-sitter-c based).
///
/// MVP scope (Stage 1): emits function definitions, function prototypes,
/// structs / unions / enums (with enumerator sub-symbols), typedefs,
/// `#define` macros and global variables. No cross-file `.h ↔ .c` join
/// (Stage 1c) and no FFI cross-language resolution (Stage 2) yet.
///
/// Package detection is the workspace-directory-name fallback today;
/// integration with `standarbuild-detect`'s C project label arrives once
/// the cold-start passes the project root via `ExtractContext`.
#[derive(Debug, Default)]
pub struct CProvider;

impl CProvider {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl LanguageProvider for CProvider {
    fn extract(
        &self,
        content: &str,
        path: &str,
        ctx: &ExtractContext<'_>,
    ) -> Result<ExtractedFile, ExtractError> {
        let package_name = helpers::workspace_dir_name(ctx.workspace_root);
        // MVP: package_root = workspace_root; later we'll honor the
        // detected project root from standarbuild-detect.
        extract::extract_file(content, path, &package_name, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use standardoc_ir::{EdgeKind, Kind, Language, ResolvedOrUnresolved, Visibility};
    use std::path::Path;

    fn run(source: &str, path: &str) -> ExtractedFile {
        let provider = CProvider::new();
        let ctx = ExtractContext {
            workspace_root: Path::new("/tmp/lurlang"),
        };
        provider.extract(source, path, &ctx).expect("extract ok")
    }

    fn find<'a>(file: &'a ExtractedFile, fqdn: &str) -> &'a standardoc_ir::RawSymbol {
        file.symbols
            .iter()
            .find(|s| s.fqdn == fqdn)
            .unwrap_or_else(|| panic!("symbol {fqdn} not found, got: {:?}", file.symbols))
    }

    #[test]
    fn extracts_function_definition_as_fn_kind() {
        let src = "int lur_vm_run(int x) { return x + 1; }\n";
        let file = run(src, "runtime/vm.c");
        let s = find(&file, "lurlang::runtime::vm::lur_vm_run");
        assert_eq!(s.kind, Kind::Function);
        assert_eq!(s.language_kind.as_str(), "fn");
        assert_eq!(s.visibility, Visibility::Public);
        assert!(s.body_hash.is_some());
        assert_eq!(file.language, Language::C);
    }

    #[test]
    fn static_function_is_private_visibility() {
        let src = "static void internal_reset(int* p) { *p = 0; }\n";
        let file = run(src, "runtime/vm.c");
        let s = find(&file, "lurlang::runtime::vm::internal_reset");
        assert_eq!(s.visibility, Visibility::Private);
    }

    #[test]
    fn function_prototype_in_header_is_fn_decl() {
        let src = "int lur_compile(const char* src);\n";
        let file = run(src, "include/lur.h");
        let s = find(&file, "lurlang::include::lur::lur_compile");
        assert_eq!(s.kind, Kind::Function);
        assert_eq!(s.language_kind.as_str(), "fn_decl");
        assert!(s.body_hash.is_none());
    }

    #[test]
    fn named_struct_emitted_as_type() {
        let src = "struct LurVm { int magic; char* program; };\n";
        let file = run(src, "runtime/vm.h");
        let s = find(&file, "lurlang::runtime::vm::LurVm");
        assert_eq!(s.kind, Kind::Type);
        assert_eq!(s.language_kind.as_str(), "struct");
    }

    #[test]
    fn enum_emits_parent_and_variants_as_sub_symbols() {
        let src = "enum LurColor { RED, GREEN, BLUE };\n";
        let file = run(src, "runtime/colors.h");
        let parent = find(&file, "lurlang::runtime::colors::LurColor");
        assert_eq!(parent.kind, Kind::Type);
        assert_eq!(parent.language_kind.as_str(), "enum");

        for variant in ["RED", "GREEN", "BLUE"] {
            let fqdn = format!("lurlang::runtime::colors::LurColor::{variant}");
            let v = find(&file, &fqdn);
            assert_eq!(v.kind, Kind::Value);
            assert_eq!(v.language_kind.as_str(), "enum_variant");
        }
    }

    #[test]
    fn typedef_emits_type_alias() {
        let src = "typedef struct LurVm LurVm;\n";
        let file = run(src, "runtime/vm.h");
        let s = find(&file, "lurlang::runtime::vm::LurVm");
        // Both the struct decl AND the typedef alias hit here — the typedef
        // wins by name. The point: the symbol exists and is queryable.
        assert_eq!(s.kind, Kind::Type);
    }

    #[test]
    fn macros_emit_with_correct_kind_and_language_kind() {
        let src = "#define MAX_FOO 100\n#define MIN(a, b) ((a) < (b) ? (a) : (b))\n";
        let file = run(src, "include/lur.h");
        let obj = find(&file, "lurlang::include::lur::MAX_FOO");
        assert_eq!(obj.kind, Kind::Macro);
        assert_eq!(obj.language_kind.as_str(), "macro_object");
        let func = find(&file, "lurlang::include::lur::MIN");
        assert_eq!(func.kind, Kind::Macro);
        assert_eq!(func.language_kind.as_str(), "macro_fn");
    }

    #[test]
    fn ifdef_guarded_content_is_still_indexed() {
        // Headers wrap content in `#ifndef X / #define X / ... / #endif`.
        // The walker must descend into preproc_ifdef so the symbols inside
        // are reachable.
        let src = "#ifndef LUR_H\n#define LUR_H\nint lur_init(void);\n#endif\n";
        let file = run(src, "include/lur.h");
        let _ = find(&file, "lurlang::include::lur::lur_init");
    }

    #[test]
    fn module_symbol_is_emitted_first() {
        let src = "int foo(void);\n";
        let file = run(src, "runtime/vm.c");
        assert!(!file.symbols.is_empty());
        let first = &file.symbols[0];
        assert_eq!(first.kind, Kind::Module);
        assert_eq!(first.fqdn, "lurlang::runtime::vm");
    }

    #[test]
    fn system_include_emits_resolved_builtin_edge() {
        let src = "#include <stdio.h>\nint foo(void) { return 0; }\n";
        let file = run(src, "runtime/vm.c");
        let edge = file
            .edges
            .iter()
            .find(|e| e.kind == EdgeKind::Imports)
            .expect("imports edge missing");
        assert_eq!(edge.from_fqdn, "lurlang::runtime::vm");
        match &edge.to {
            ResolvedOrUnresolved::Resolved { fqdn } => {
                assert_eq!(fqdn, "<builtin>::c::stdio");
            }
            other => panic!("expected Resolved builtin, got {other:?}"),
        }
    }

    #[test]
    fn local_include_emits_unresolved_edge_with_path() {
        let src = "#include \"runtime/util.h\"\nint foo(void);\n";
        let file = run(src, "runtime/vm.c");
        let edge = file
            .edges
            .iter()
            .find(|e| e.kind == EdgeKind::Imports)
            .expect("imports edge missing");
        match &edge.to {
            ResolvedOrUnresolved::Unresolved { name } => {
                assert_eq!(name, "runtime/util.h");
            }
            other => panic!("expected Unresolved by name, got {other:?}"),
        }
    }

    #[test]
    fn multiple_includes_all_get_edges() {
        let src = "#include <stdlib.h>\n#include <string.h>\n#include \"foo.h\"\n";
        let file = run(src, "main.c");
        let imports: Vec<_> = file
            .edges
            .iter()
            .filter(|e| e.kind == EdgeKind::Imports)
            .collect();
        assert_eq!(imports.len(), 3);
    }
}


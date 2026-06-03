use standardoc_core::{ExtractContext, ExtractError, LanguageProvider};
use standardoc_ir::ExtractedFile;

mod call_sites;
mod extract;
mod helpers;
mod lookup;
mod lua_export_tagger;
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
    pub const fn new() -> Self {
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
mod tests;

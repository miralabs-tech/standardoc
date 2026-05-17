use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use standardoc_core::{ExtractContext, ExtractError, LanguageProvider};
use standardoc_ir::ExtractedFile;

mod emmylua;
mod extract;
mod extract_doc;
mod ffi_tagger;
mod helpers;
mod lookup;
mod resolver;
mod walk;

/// Native Lua `LanguageProvider` (full_moon-based).
///
/// Mirrors `RustProvider` / `TsProvider`: each `.lua` file is parsed via
/// `full_moon`, the nearest `*.rockspec` ancestor is located (if any) and
/// parsed for the package name (FQDN namespace); without a rockspec, the
/// workspace directory name is used as fallback. Items + bodies are walked
/// for symbols / edges. The package name is cached per-anchor in a
/// `RwLock<HashMap>` keyed by the canonical rockspec path or workspace root.
#[derive(Debug, Default)]
pub struct LuaProvider {
    package_name_cache: RwLock<HashMap<PathBuf, String>>,
}

impl LuaProvider {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Resolves the FQDN package namespace for a given Lua source file.
    /// Returns `(package_name, package_root)` where `package_root` is the
    /// directory used to compute the file's `package_relative` path.
    ///
    /// Lookup order:
    /// 1. Walk up from `file_abs_path` looking for a `*.rockspec`. If
    ///    found, parse its `package = "..."` literal — that's the
    ///    namespace. `package_root` = rockspec parent directory.
    /// 2. Fall back to the workspace directory name. `package_root` =
    ///    `workspace_root`.
    fn resolve_package_name(
        &self,
        file_abs_path: &Path,
        workspace_root: &Path,
    ) -> Result<(String, PathBuf), ExtractError> {
        if let Some(rockspec_path) = helpers::find_rockspec(file_abs_path, workspace_root) {
            let rockspec_root = rockspec_path
                .parent()
                .map_or_else(|| workspace_root.to_path_buf(), Path::to_path_buf);

            if let Some(hit) = self
                .package_name_cache
                .read()
                .ok()
                .and_then(|guard| guard.get(&rockspec_path).cloned())
            {
                return Ok((hit, rockspec_root));
            }

            let content = std::fs::read_to_string(&rockspec_path).map_err(ExtractError::Io)?;
            if let Some(name) = helpers::parse_rockspec_name(&content) {
                if let Ok(mut guard) = self.package_name_cache.write() {
                    guard.insert(rockspec_path, name.clone());
                }
                return Ok((name, rockspec_root));
            }
            // Rockspec found but unparseable `package` field — fall through
            // to workspace fallback rather than failing the file.
        }

        // No rockspec (or unparseable) — workspace_dir_name fallback.
        let key = workspace_root.to_path_buf();
        if let Some(hit) = self
            .package_name_cache
            .read()
            .ok()
            .and_then(|guard| guard.get(&key).cloned())
        {
            return Ok((hit, key));
        }

        let name = helpers::workspace_dir_name(workspace_root);
        if let Ok(mut guard) = self.package_name_cache.write() {
            guard.insert(key.clone(), name.clone());
        }
        Ok((name, key))
    }
}

impl LanguageProvider for LuaProvider {
    fn extract(
        &self,
        content: &str,
        path: &str,
        ctx: &ExtractContext<'_>,
    ) -> Result<ExtractedFile, ExtractError> {
        let file_abs_path = ctx.workspace_root.join(path);
        let (package_name, package_root) =
            self.resolve_package_name(&file_abs_path, ctx.workspace_root)?;
        let package_relative = file_abs_path.strip_prefix(&package_root).map_or_else(
            |_| path.to_string(),
            |p| p.to_string_lossy().replace('\\', "/"),
        );
        extract::extract_file(
            content,
            path,
            &package_name,
            &package_relative,
            &file_abs_path,
            &package_root,
        )
    }
}

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use standardoc_core::{ExtractContext, ExtractError, LanguageProvider};
use standardoc_ir::ExtractedFile;

mod body_hash;
mod crate_root;
mod extract;
mod extract_call;
mod extract_use;
mod module_path;
mod visibility;
mod walk;

/// Native Rust `LanguageProvider` (syn 2-based).
///
/// Parses each `.rs` file via `syn::parse_file`, computes its module path
/// from the workspace-relative path + the parent crate's `Cargo.toml`
/// `[package].name`, then walks items + bodies for symbols/edges.
///
/// The parent crate name is resolved by walking up the filesystem from the
/// file's absolute path until a `Cargo.toml` with `[package].name` is found.
/// Results are cached in a per-Cargo.toml `RwLock<HashMap>` keyed by the
/// canonical Cargo.toml path so a 50k-file cold start performs the I/O once
/// per crate.
#[derive(Debug, Default)]
pub struct RustProvider {
    crate_name_cache: RwLock<HashMap<PathBuf, String>>,
}

impl RustProvider {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn resolve_crate_name(
        &self,
        file_abs_path: &Path,
        workspace_relative: &str,
    ) -> Result<String, ExtractError> {
        let cargo_toml = crate_root::find_cargo_toml(file_abs_path).ok_or_else(|| {
            ExtractError::Parse {
                file: workspace_relative.into(),
                detail: "could not determine crate name (no Cargo.toml ancestor)".into(),
            }
        })?;

        if let Some(hit) = self
            .crate_name_cache
            .read()
            .ok()
            .and_then(|guard| guard.get(&cargo_toml).cloned())
        {
            return Ok(hit);
        }

        let toml_content =
            std::fs::read_to_string(&cargo_toml).map_err(ExtractError::Io)?;
        let crate_name = crate_root::parse_package_name(&toml_content).ok_or_else(|| {
            ExtractError::Parse {
                file: workspace_relative.into(),
                detail: "could not determine crate name (Cargo.toml has no [package].name)"
                    .into(),
            }
        })?;

        if let Ok(mut guard) = self.crate_name_cache.write() {
            guard.insert(cargo_toml, crate_name.clone());
        }
        Ok(crate_name)
    }
}

impl LanguageProvider for RustProvider {
    fn extract(
        &self,
        content: &str,
        path: &str,
        ctx: &ExtractContext<'_>,
    ) -> Result<ExtractedFile, ExtractError> {
        let file_abs_path = ctx.workspace_root.join(path);
        let crate_name = self.resolve_crate_name(&file_abs_path, path)?;
        extract::extract_file(content, path, &crate_name)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::sync::Arc;

    use standardoc_core::{ExtractContext, ExtractError, LanguageProvider};
    use tempfile::tempdir;

    use super::RustProvider;

    fn write(root: &Path, rel: &str, content: &str) {
        let abs = root.join(rel);
        if let Some(parent) = abs.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(abs, content).unwrap();
    }

    #[test]
    fn extract_resolves_crate_name_from_cargo_toml() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        write(root, "Cargo.toml", "[package]\nname = \"mycrate\"\nversion = \"0.1.0\"\n");
        write(root, "src/lib.rs", "pub fn foo() {}\n");

        let provider = RustProvider::new();
        let ctx = ExtractContext { workspace_root: root };
        let extracted = provider
            .extract("pub fn foo() {}\n", "src/lib.rs", &ctx)
            .expect("extract ok");

        // file Module symbol is the first symbol; its fqdn = crate name (lib.rs root).
        let module = &extracted.symbols[0];
        assert_eq!(module.fqdn, "mycrate");
        let foo = extracted
            .symbols
            .iter()
            .find(|s| s.name == "foo")
            .expect("foo symbol");
        assert_eq!(foo.fqdn, "mycrate::foo");
    }

    #[test]
    fn extract_returns_parse_error_when_no_cargo_toml() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        write(root, "src/lib.rs", "fn foo() {}\n");

        let provider = RustProvider::new();
        let ctx = ExtractContext { workspace_root: root };
        let err = provider
            .extract("fn foo() {}\n", "src/lib.rs", &ctx)
            .expect_err("must fail without Cargo.toml");
        match err {
            ExtractError::Parse { file, detail } => {
                assert_eq!(file, "src/lib.rs");
                assert!(detail.contains("Cargo.toml"));
            }
            other => panic!("expected Parse error, got {other:?}"),
        }
    }

    #[test]
    fn extract_returns_parse_error_when_cargo_toml_has_no_package_name() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        write(root, "Cargo.toml", "[workspace]\nmembers = [\"crates/*\"]\n");
        write(root, "src/lib.rs", "fn foo() {}\n");

        let provider = RustProvider::new();
        let ctx = ExtractContext { workspace_root: root };
        let err = provider
            .extract("fn foo() {}\n", "src/lib.rs", &ctx)
            .expect_err("must fail without [package].name");
        match err {
            ExtractError::Parse { file, detail } => {
                assert_eq!(file, "src/lib.rs");
                assert!(detail.contains("[package].name"));
            }
            other => panic!("expected Parse error, got {other:?}"),
        }
    }

    #[test]
    fn cache_hit_avoids_repeated_io_for_same_cargo_toml() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        write(root, "Cargo.toml", "[package]\nname = \"foo\"\n");
        write(root, "src/lib.rs", "pub fn a() {}\n");
        write(root, "src/bar.rs", "pub fn b() {}\n");

        let provider = RustProvider::new();
        let ctx = ExtractContext { workspace_root: root };

        let _ = provider
            .extract("pub fn a() {}\n", "src/lib.rs", &ctx)
            .unwrap();

        // Now overwrite the Cargo.toml: a cache hit must keep returning "foo".
        write(root, "Cargo.toml", "[package]\nname = \"DIFFERENT\"\n");

        let extracted_b = provider
            .extract("pub fn b() {}\n", "src/bar.rs", &ctx)
            .unwrap();
        let module_b = &extracted_b.symbols[0];
        assert_eq!(module_b.fqdn, "foo::bar");
    }

    #[test]
    fn provider_is_send_sync_via_arc() {
        let provider: Arc<dyn LanguageProvider> = Arc::new(RustProvider::new());
        // Compile-time assertion via Arc<dyn LanguageProvider> (trait requires Send + Sync).
        let _ = provider.clone();
    }
}

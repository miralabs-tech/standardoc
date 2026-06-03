use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::SystemTime;

use standardoc_core::{ExtractContext, ExtractError, LanguageProvider};
use standardoc_ir::ExtractedFile;

use self::collect_globals::{WorkspaceFile, collect_global_returns};
use self::global_return_type_registry::GlobalReturnTypeRegistry;

mod body_hash;
mod collect_globals;
mod crate_root;
mod derive_impls;
mod extract;
mod extract_call;
mod extract_doc;
mod extract_type;
mod extract_use;
mod ffi_tagger;
mod global_return_type_registry;
mod lookup;
mod module_path;
mod return_type_table;
mod struct_field_table;
mod type_name;
mod visibility;
mod walk;

pub(crate) use ffi_tagger::extract_ffi_bindings;

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
/// per crate. The cached value carries the Cargo.toml's modification time;
/// a cache hit only counts when the on-disk mtime still matches, so renaming
/// `[package].name` is picked up on the next extraction instead of staying
/// stale until a daemon restart.
#[derive(Debug, Default)]
pub struct RustProvider {
    crate_name_cache: RwLock<HashMap<PathBuf, (SystemTime, String)>>,
    /// Workspace-global return-type registry populated by
    /// `workspace_prepare`. `None` until the first cold-start /
    /// watcher pre-pass runs, OR for `RustProvider::new()` instances
    /// used directly in tests that don't go through `cold_start`.
    /// `Arc` so per-file `extract` calls hand a cheap clone to the
    /// `WalkContext` without locking on each lookup.
    global_return_types: RwLock<Option<Arc<GlobalReturnTypeRegistry>>>,
}

impl RustProvider {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Resolves both the crate name AND the crate root absolute path for
    /// the file at `file_abs_path`. The crate root = parent directory of
    /// the closest `Cargo.toml` ancestor. Used by `extract` to compute a
    /// crate-root-relative path for module-path derivation when the
    /// workspace_root is at or above the crate root (monorepo layout
    /// where the user opens the parent dir in VSCode).
    fn resolve_crate_info(
        &self,
        file_abs_path: &Path,
        workspace_relative: &str,
    ) -> Result<(String, PathBuf), ExtractError> {
        let cargo_toml =
            crate_root::find_cargo_toml(file_abs_path).ok_or_else(|| ExtractError::Parse {
                file: workspace_relative.into(),
                detail: "could not determine crate name (no Cargo.toml ancestor)".into(),
            })?;
        let crate_root_abs = cargo_toml
            .parent()
            .ok_or_else(|| ExtractError::Parse {
                file: workspace_relative.into(),
                detail: "Cargo.toml ancestor has no parent directory".into(),
            })?
            .to_path_buf();

        // Self-invalidating on the Cargo.toml mtime: a hit only counts when
        // the file hasn't been touched since we cached it, so a
        // `[package].name` rename is picked up on the next extraction. A
        // missing mtime (rare I/O failure) skips the cache rather than risking
        // a stale read.
        let mtime = std::fs::metadata(&cargo_toml)
            .and_then(|m| m.modified())
            .ok();
        if let Some(mtime) = mtime
            && let Some(hit) = self
                .crate_name_cache
                .read()
                .ok()
                .and_then(|guard| guard.get(&cargo_toml).cloned())
                .filter(|(cached_mtime, _)| *cached_mtime == mtime)
                .map(|(_, name)| name)
        {
            return Ok((hit, crate_root_abs));
        }

        let toml_content = std::fs::read_to_string(&cargo_toml).map_err(ExtractError::Io)?;
        let crate_name =
            crate_root::parse_package_name(&toml_content).ok_or_else(|| ExtractError::Parse {
                file: workspace_relative.into(),
                detail: "could not determine crate name (Cargo.toml has no [package].name)".into(),
            })?;

        if let Some(mtime) = mtime
            && let Ok(mut guard) = self.crate_name_cache.write()
        {
            guard.insert(cargo_toml, (mtime, crate_name.clone()));
        }
        Ok((crate_name, crate_root_abs))
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
        let (crate_name, crate_root_abs) = self.resolve_crate_info(&file_abs_path, path)?;
        // Module-path derivation must operate on the path RELATIVE to the
        // crate root, not the workspace root. When the user opens a
        // parent directory (monorepo layout), the workspace-relative
        // path would otherwise leak path segments like
        // `crates/<crate>/src/...` into the FQDN
        // (`<crate>::crates::<crate>::...`). Symbol locations stay
        // workspace-relative — only `module_path::compute` consumes
        // the crate-rel path.
        let crate_rel = file_abs_path.strip_prefix(&crate_root_abs).map_or_else(
            |_| path.to_string(),
            |p| p.to_string_lossy().replace('\\', "/"),
        );
        let global_returns = self
            .global_return_types
            .read()
            .ok()
            .and_then(|guard| guard.as_ref().cloned());
        extract::extract_file(content, path, &crate_name, &crate_rel, global_returns)
    }

    fn workspace_prepare(&self, workspace_root: &Path, rel_paths: &[String]) {
        // Pass 0 — pre-pass over every `.rs` file in the workspace to
        // populate `global_return_types`. Pure I/O + parse, no edge
        // emission, no symbol push. The `cold_start` orchestrator (and
        // the watcher on file-changes) calls this once before the
        // per-file `extract` loop so cross-file fn-return-type lookups
        // succeed during Pass 1.
        let mut slots: Vec<RustWorkspaceSlot> = Vec::with_capacity(rel_paths.len());
        for rel in rel_paths {
            let is_rust = Path::new(rel)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("rs"));
            if !is_rust {
                continue;
            }
            let abs = workspace_root.join(rel);
            let Ok(content) = std::fs::read_to_string(&abs) else {
                continue;
            };
            let Ok((crate_name, crate_root_abs)) = self.resolve_crate_info(&abs, rel) else {
                continue;
            };
            let crate_rel = abs
                .strip_prefix(&crate_root_abs)
                .map_or_else(|_| rel.clone(), |p| p.to_string_lossy().replace('\\', "/"));
            slots.push(RustWorkspaceSlot {
                crate_name,
                crate_rel,
                content,
            });
        }
        let files: Vec<WorkspaceFile<'_>> = slots
            .iter()
            .map(|s| WorkspaceFile {
                crate_name: &s.crate_name,
                crate_rel: &s.crate_rel,
                content: &s.content,
            })
            .collect();
        let registry = collect_global_returns(&files);
        if let Ok(mut guard) = self.global_return_types.write() {
            *guard = Some(Arc::new(registry));
        }
    }
}

/// Owns the (crate_name, crate_rel, content) tuple for one file
/// during `workspace_prepare`. The `WorkspaceFile` slice handed to
/// `collect_global_returns` borrows from this, so we keep the owners
/// alive in a parallel `Vec` and zip them through `&` references at
/// the call site.
struct RustWorkspaceSlot {
    crate_name: String,
    crate_rel: String,
    content: String,
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
        write(
            root,
            "Cargo.toml",
            "[package]\nname = \"mycrate\"\nversion = \"0.1.0\"\n",
        );
        write(root, "src/lib.rs", "pub fn foo() {}\n");

        let provider = RustProvider::new();
        let ctx = ExtractContext {
            workspace_root: root,
            cross_workspace: None,
        };
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
        let ctx = ExtractContext {
            workspace_root: root,
            cross_workspace: None,
        };
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
        write(
            root,
            "Cargo.toml",
            "[workspace]\nmembers = [\"crates/*\"]\n",
        );
        write(root, "src/lib.rs", "fn foo() {}\n");

        let provider = RustProvider::new();
        let ctx = ExtractContext {
            workspace_root: root,
            cross_workspace: None,
        };
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
    fn cache_hit_reuses_crate_name_when_cargo_toml_unchanged() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        write(root, "Cargo.toml", "[package]\nname = \"foo\"\n");

        let provider = RustProvider::new();
        let ctx = ExtractContext {
            workspace_root: root,
            cross_workspace: None,
        };

        // Two files in the same crate: the second resolves its crate name
        // from the cache (same untouched Cargo.toml).
        let a = provider
            .extract("pub fn a() {}\n", "src/lib.rs", &ctx)
            .unwrap();
        let b = provider
            .extract("pub fn b() {}\n", "src/bar.rs", &ctx)
            .unwrap();
        assert_eq!(a.symbols[0].fqdn, "foo");
        assert_eq!(b.symbols[0].fqdn, "foo::bar");
    }

    #[test]
    fn cache_self_invalidates_when_cargo_toml_is_renamed() {
        use std::time::{Duration, SystemTime};

        let dir = tempdir().unwrap();
        let root = dir.path();
        let cargo = root.join("Cargo.toml");
        write(root, "Cargo.toml", "[package]\nname = \"foo\"\n");

        let provider = RustProvider::new();
        let ctx = ExtractContext {
            workspace_root: root,
            cross_workspace: None,
        };

        let a = provider
            .extract("pub fn a() {}\n", "src/lib.rs", &ctx)
            .unwrap();
        assert_eq!(a.symbols[0].fqdn, "foo");

        // Rename the package and force a strictly-newer mtime so the cache
        // self-invalidates deterministically, regardless of the filesystem's
        // mtime resolution.
        write(root, "Cargo.toml", "[package]\nname = \"renamed\"\n");
        fs::File::options()
            .write(true)
            .open(&cargo)
            .unwrap()
            .set_modified(SystemTime::now() + Duration::from_secs(5))
            .unwrap();

        let b = provider
            .extract("pub fn b() {}\n", "src/bar.rs", &ctx)
            .unwrap();
        assert_eq!(b.symbols[0].fqdn, "renamed::bar");
    }

    #[test]
    fn extract_tags_main_at_crate_root_as_binary_main_entry_point() {
        use standardoc_ir::EntryPointKind;
        let dir = tempdir().unwrap();
        let root = dir.path();
        write(
            root,
            "Cargo.toml",
            "[package]\nname = \"runme\"\nversion = \"0.1.0\"\n",
        );
        let src = "fn main() {}\nfn helper() {}\npub fn other_top_level() {}\n";
        write(root, "src/main.rs", src);

        let provider = RustProvider::new();
        let ctx = ExtractContext {
            workspace_root: root,
            cross_workspace: None,
        };
        let extracted = provider.extract(src, "src/main.rs", &ctx).unwrap();

        let main_sym = extracted
            .symbols
            .iter()
            .find(|s| s.name == "main")
            .expect("main symbol");
        assert_eq!(main_sym.entry_point, Some(EntryPointKind::BinaryMain));

        // Sibling free functions at crate root must NOT be tagged.
        for name in ["helper", "other_top_level"] {
            let s = extracted
                .symbols
                .iter()
                .find(|s| s.name == name)
                .unwrap_or_else(|| panic!("{name} symbol"));
            assert_eq!(s.entry_point, None, "{name} must not be an entry-point");
        }
    }

    #[test]
    fn extract_tags_no_mangle_fn_as_ffi_export_entry_point() {
        use standardoc_ir::EntryPointKind;
        let dir = tempdir().unwrap();
        let root = dir.path();
        write(
            root,
            "Cargo.toml",
            "[package]\nname = \"bridged\"\nversion = \"0.1.0\"\n",
        );
        let src = "#[no_mangle]\npub extern \"C\" fn exported() {}\npub fn plain() {}\n";
        write(root, "src/lib.rs", src);

        let provider = RustProvider::new();
        let ctx = ExtractContext {
            workspace_root: root,
            cross_workspace: None,
        };
        let extracted = provider.extract(src, "src/lib.rs", &ctx).unwrap();

        let exp = extracted
            .symbols
            .iter()
            .find(|s| s.name == "exported")
            .expect("exported symbol");
        assert_eq!(exp.entry_point, Some(EntryPointKind::FfiExport));

        let plain = extracted
            .symbols
            .iter()
            .find(|s| s.name == "plain")
            .expect("plain symbol");
        assert_eq!(plain.entry_point, None);
    }

    #[test]
    fn extract_does_not_tag_nested_main_as_binary_main() {
        use standardoc_ir::EntryPointKind;
        // A `fn main` nested in a submodule is NOT the program entry-point —
        // only the crate-root one is. This guards against false positives
        // in test fixtures, embedded examples, and `mod tests { fn main … }`.
        let dir = tempdir().unwrap();
        let root = dir.path();
        write(
            root,
            "Cargo.toml",
            "[package]\nname = \"nested\"\nversion = \"0.1.0\"\n",
        );
        let src = "pub mod inner { pub fn main() {} }\n";
        write(root, "src/lib.rs", src);

        let provider = RustProvider::new();
        let ctx = ExtractContext {
            workspace_root: root,
            cross_workspace: None,
        };
        let extracted = provider.extract(src, "src/lib.rs", &ctx).unwrap();
        let nested = extracted
            .symbols
            .iter()
            .find(|s| s.name == "main" && s.module.as_deref() == Some("nested::inner"))
            .expect("nested main symbol");
        assert_ne!(nested.entry_point, Some(EntryPointKind::BinaryMain));
    }

    #[test]
    fn provider_is_send_sync_via_arc() {
        let provider: Arc<dyn LanguageProvider> = Arc::new(RustProvider::new());
        // Compile-time assertion via Arc<dyn LanguageProvider> (trait requires Send + Sync).
        let _ = provider.clone();
    }
}

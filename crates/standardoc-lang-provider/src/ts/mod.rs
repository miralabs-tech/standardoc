use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use standardoc_core::{ExtractContext, ExtractError, LanguageProvider};
use standardoc_ir::{ExtractedFile, Language};
use swc_core::ecma::parser::Syntax;

mod extract;
mod extract_doc;
mod helpers;
mod lookup;
mod resolver;
mod visit;
mod walk;

/// Native TypeScript / JavaScript `LanguageProvider` (swc-based).
///
/// Mirrors `RustProvider`: each `.ts/.tsx/.js/.jsx` file is parsed via
/// `swc_ecma_parser`, the parent `package.json` is located and parsed for the
/// package name (FQDN namespace), then items + bodies are walked for
/// symbols / edges. The package name is cached in a per-package.json
/// `RwLock<HashMap>` keyed by the canonical package.json path.
#[derive(Debug, Default)]
pub struct TsProvider {
    package_name_cache: RwLock<HashMap<PathBuf, String>>,
}

impl TsProvider {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn resolve_package_name(
        &self,
        file_abs_path: &Path,
        workspace_relative: &str,
    ) -> Result<(String, PathBuf), ExtractError> {
        let package_json =
            helpers::find_package_json(file_abs_path).ok_or_else(|| ExtractError::Parse {
                file: workspace_relative.into(),
                detail: "could not determine package name (no package.json ancestor)".into(),
            })?;

        if let Some(hit) = self
            .package_name_cache
            .read()
            .ok()
            .and_then(|guard| guard.get(&package_json).cloned())
        {
            let root = package_json
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_default();
            return Ok((hit, root));
        }

        let content = std::fs::read_to_string(&package_json).map_err(ExtractError::Io)?;
        let name = helpers::parse_package_name(&content).ok_or_else(|| ExtractError::Parse {
            file: workspace_relative.into(),
            detail: "could not determine package name (package.json has no \"name\" field)".into(),
        })?;

        if let Ok(mut guard) = self.package_name_cache.write() {
            guard.insert(package_json.clone(), name.clone());
        }
        let root = package_json
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default();
        Ok((name, root))
    }
}

impl LanguageProvider for TsProvider {
    fn extract(
        &self,
        content: &str,
        path: &str,
        ctx: &ExtractContext<'_>,
    ) -> Result<ExtractedFile, ExtractError> {
        self.extract_with_overrides(content, path, ctx, None, None)
    }
}

/// Walk mode threaded through the TS provider's visit layer (scaffold A
/// surface, scaffold B impl). `Workspace` is the historical full walk —
/// CALLS edges, body_hash, IMPLEMENTS, the works. `ExternalDts` is the
/// stripped-down mode for `node_modules/<pkg>/*.d.ts` declarations:
/// no bodies (declaration files have none), so no CALLS, no body_hash;
/// IMPORTS + IMPLEMENTS + types stay (they are exactly what the agent
/// wants for cross-package navigation).
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum TsExtractMode {
    /// Workspace files. Full walk — every edge kind, body hashing,
    /// EmmyLua-style JSDoc parsing (post-beta), JsxRefVisitor on bodies.
    #[default]
    Workspace,
    /// External `.d.ts` (and `.ts`/`.js` source) from `node_modules`.
    /// The walk skips body-dependent emissions: no CALLS edges, no
    /// body_hash, no JsxRefVisitor on function bodies (declarations
    /// have none). Doc extraction stays — `.d.ts` files routinely
    /// carry the only authoritative JSDoc for a package.
    ExternalDts,
}

impl TsProvider {
    /// SFC-orchestrator entry point (lock 41 §2.5). Routes through the
    /// same package/tsconfig discovery as [`Self::extract`] but lets the
    /// caller force the swc syntax (e.g. `Syntax::Typescript{tsx:false,..}`
    /// for a `.vue` `<script lang="ts">` payload) and stamp a different
    /// IR language tag on the resulting `ExtractedFile`
    /// (e.g. `Language::Vue` even though the body was parsed as TS).
    pub(crate) fn extract_with_overrides(
        &self,
        content: &str,
        path: &str,
        ctx: &ExtractContext<'_>,
        syntax_override: Option<Syntax>,
        language_override: Option<Language>,
    ) -> Result<ExtractedFile, ExtractError> {
        let file_abs_path = ctx.workspace_root.join(path);
        let (package_name, package_root) = self.resolve_package_name(&file_abs_path, path)?;
        let package_relative = file_abs_path.strip_prefix(&package_root).map_or_else(
            |_| path.to_string(),
            |p| p.to_string_lossy().replace('\\', "/"),
        );
        let tsconfig = load_nearest_tsconfig(&file_abs_path, &package_root);
        extract::extract_file_with_syntax(
            content,
            path,
            &package_name,
            &package_relative,
            &file_abs_path,
            &package_root,
            tsconfig,
            syntax_override,
            language_override,
        )
    }
}

impl TsProvider {
    /// External-`.d.ts` entry point used by `externals::npm::NpmResolver`.
    /// Sets `TsExtractMode::ExternalDts` (crate-private) so the visit layer skips
    /// CALLS edges + body hashing for declaration files. The path is
    /// treated as a workspace-relative pointer into a virtual
    /// `node_modules/<pkg>/...` tree mounted under `ctx.workspace_root`;
    /// the package name is resolved via the package's own `package.json`
    /// the way [`Self::extract`] does for workspace files.
    pub fn extract_external_dts(
        &self,
        content: &str,
        path: &str,
        ctx: &ExtractContext<'_>,
    ) -> Result<ExtractedFile, ExtractError> {
        let _ = (content, path, ctx);
        todo!(
            "scaffold A — thread TsExtractMode::ExternalDts through extract_with_overrides (scaffold B impl)"
        )
    }
}

fn load_nearest_tsconfig(from_file: &Path, package_root: &Path) -> Option<resolver::TsConfigPaths> {
    let mut current = from_file.parent()?;
    loop {
        let candidate = current.join("tsconfig.json");
        if candidate.is_file()
            && let Ok(content) = std::fs::read_to_string(&candidate)
            && let Some(cfg) = resolver::parse_tsconfig(&content)
        {
            return Some(cfg);
        }
        if current == package_root {
            return None;
        }
        current = current.parent()?;
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::sync::Arc;

    use standardoc_core::{ExtractContext, ExtractError, LanguageProvider};
    use tempfile::tempdir;

    use super::TsProvider;

    fn write(root: &Path, rel: &str, content: &str) {
        let abs = root.join(rel);
        if let Some(parent) = abs.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(abs, content).unwrap();
    }

    #[test]
    fn extract_resolves_package_name_from_package_json() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        write(root, "package.json", r#"{"name":"@myorg/api"}"#);
        let src = "export function foo(): void {}\n";
        write(root, "src/index.ts", src);

        let provider = TsProvider::new();
        let ctx = ExtractContext {
            workspace_root: root,
            cross_workspace: None,
        };
        let extracted = provider
            .extract(src, "src/index.ts", &ctx)
            .expect("extract ok");

        let module = &extracted.symbols[0];
        assert_eq!(module.fqdn, "@myorg/api::src");
        let foo = extracted
            .symbols
            .iter()
            .find(|s| s.name == "foo")
            .expect("foo symbol");
        assert_eq!(foo.fqdn, "@myorg/api::src::foo");
    }

    #[test]
    fn extract_returns_parse_error_when_no_package_json() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let src = "export function foo() {}\n";
        write(root, "src/index.ts", src);

        let provider = TsProvider::new();
        let ctx = ExtractContext {
            workspace_root: root,
            cross_workspace: None,
        };
        let err = provider
            .extract(src, "src/index.ts", &ctx)
            .expect_err("must fail without package.json");
        match err {
            ExtractError::Parse { file, detail } => {
                assert_eq!(file, "src/index.ts");
                assert!(detail.contains("package.json"));
            }
            other => panic!("expected Parse error, got {other:?}"),
        }
    }

    #[test]
    fn extract_returns_parse_error_when_package_json_has_no_name() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        write(root, "package.json", r#"{"private": true}"#);
        let src = "export function foo() {}\n";
        write(root, "src/index.ts", src);

        let provider = TsProvider::new();
        let ctx = ExtractContext {
            workspace_root: root,
            cross_workspace: None,
        };
        let err = provider
            .extract(src, "src/index.ts", &ctx)
            .expect_err("must fail without name");
        match err {
            ExtractError::Parse { file, detail } => {
                assert_eq!(file, "src/index.ts");
                assert!(detail.contains("\"name\""));
            }
            other => panic!("expected Parse error, got {other:?}"),
        }
    }

    #[test]
    fn cache_hit_avoids_repeated_io_for_same_package_json() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        write(root, "package.json", r#"{"name":"foo"}"#);
        write(root, "src/a.ts", "export const a = 1;\n");
        write(root, "src/b.ts", "export const b = 2;\n");

        let provider = TsProvider::new();
        let ctx = ExtractContext {
            workspace_root: root,
            cross_workspace: None,
        };

        let _ = provider
            .extract("export const a = 1;\n", "src/a.ts", &ctx)
            .unwrap();

        // Overwrite package.json — cache hit must keep returning "foo".
        write(root, "package.json", r#"{"name":"DIFFERENT"}"#);

        let extracted_b = provider
            .extract("export const b = 2;\n", "src/b.ts", &ctx)
            .unwrap();
        let module_b = &extracted_b.symbols[0];
        assert_eq!(module_b.fqdn, "foo::src::b");
    }

    #[test]
    fn provider_is_send_sync_via_arc() {
        let provider: Arc<dyn LanguageProvider> = Arc::new(TsProvider::new());
        let _ = provider.clone();
    }

    #[test]
    fn extract_consumes_tsconfig_paths_when_present() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        write(root, "package.json", r#"{"name":"@myorg/api"}"#);
        write(
            root,
            "tsconfig.json",
            r#"{"compilerOptions":{"baseUrl":"./","paths":{"@lib/*":["src/lib/*"]}}}"#,
        );
        let src = "import { helper } from '@lib/utils';\nexport function caller() { helper(); }\n";
        write(root, "src/caller.ts", src);

        let provider = TsProvider::new();
        let ctx = ExtractContext {
            workspace_root: root,
            cross_workspace: None,
        };
        let extracted = provider.extract(src, "src/caller.ts", &ctx).expect("ok");

        let imports: Vec<_> = extracted
            .edges
            .iter()
            .filter(|e| e.kind == standardoc_ir::EdgeKind::Imports)
            .collect();
        assert_eq!(imports.len(), 1);
        match &imports[0].to {
            standardoc_ir::ResolvedOrUnresolved::Unresolved { name } => {
                assert_eq!(name, "@myorg/api::src::lib::utils::helper");
            }
            other => panic!("expected unresolved canonical via tsconfig, got {other:?}"),
        }
    }
}

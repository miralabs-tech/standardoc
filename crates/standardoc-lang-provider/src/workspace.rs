use std::path::Path;

use standardoc_core::{ExtractContext, ExtractError, LanguageProvider};
use standardoc_ir::ExtractedFile;

use crate::rust::RustProvider;
use crate::ts::TsProvider;

#[derive(Debug, Default)]
pub struct WorkspaceProvider {
    rust: RustProvider,
    ts: TsProvider,
}

impl WorkspaceProvider {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl LanguageProvider for WorkspaceProvider {
    fn extract(
        &self,
        content: &str,
        path: &str,
        ctx: &ExtractContext<'_>,
    ) -> Result<ExtractedFile, ExtractError> {
        match dispatch(path) {
            Some(Dispatch::Rust) => self.rust.extract(content, path, ctx),
            Some(Dispatch::TypeScript) => self.ts.extract(content, path, ctx),
            None => Err(ExtractError::UnsupportedLanguage { file: path.into() }),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Dispatch {
    Rust,
    TypeScript,
}

fn dispatch(path: &str) -> Option<Dispatch> {
    let ext = Path::new(path).extension()?.to_str()?;
    match ext {
        "rs" => Some(Dispatch::Rust),
        "ts" | "tsx" | "js" | "jsx" => Some(Dispatch::TypeScript),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatch_rust_extension() {
        assert_eq!(dispatch("src/lib.rs"), Some(Dispatch::Rust));
        assert_eq!(dispatch("crates/foo/src/main.rs"), Some(Dispatch::Rust));
    }

    #[test]
    fn dispatch_typescript_extensions() {
        assert_eq!(dispatch("src/index.ts"), Some(Dispatch::TypeScript));
        assert_eq!(dispatch("src/App.tsx"), Some(Dispatch::TypeScript));
        assert_eq!(dispatch("scripts/build.js"), Some(Dispatch::TypeScript));
        assert_eq!(dispatch("src/legacy.jsx"), Some(Dispatch::TypeScript));
    }

    #[test]
    fn dispatch_unsupported_returns_none() {
        assert_eq!(dispatch("README.md"), None);
        assert_eq!(dispatch("Cargo.toml"), None);
        assert_eq!(dispatch("package.json"), None);
    }

    #[test]
    fn dispatch_no_extension_returns_none() {
        assert_eq!(dispatch("Makefile"), None);
        assert_eq!(dispatch(""), None);
    }
}

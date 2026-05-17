use std::path::Path;

use standardoc_ir::{BuiltinEntry, ExtractedFile};

pub trait LanguageProvider: Send + Sync {
    fn extract(
        &self,
        content: &str,
        path: &str,
        ctx: &ExtractContext<'_>,
    ) -> Result<ExtractedFile, ExtractError>;

    /// Stage 3e-1: builtin entries the pipeline should eagerly seed at
    /// cold-start so the synthetic FQDNs emitted by tier-aware resolvers
    /// (`<builtin>::ts::Math`, `<builtin>::lua::print`, …) resolve to a
    /// real row in `symbols`. Default returns empty — providers that
    /// expose a `BuiltinRegistry` override to surface their Edge-tier
    /// entries. Drop / Attribute tiers do NOT seed: they never produce
    /// edges in the first place, so a DB row would be unreachable.
    fn edge_builtins(&self) -> Vec<BuiltinEntry> {
        Vec::new()
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ExtractContext<'a> {
    pub workspace_root: &'a Path,
}

#[derive(Debug, thiserror::Error)]
pub enum ExtractError {
    #[error("parse error in {file}: {detail}")]
    Parse { file: String, detail: String },

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("unsupported language for {file}")]
    UnsupportedLanguage { file: String },
}

#[cfg(test)]
pub(crate) mod mock {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use super::{ExtractContext, ExtractError, ExtractedFile, LanguageProvider};

    #[derive(Debug, Clone)]
    pub(crate) enum MockResponse {
        Ok(ExtractedFile),
        ParseError(String),
    }

    pub(crate) struct MockProvider {
        responses: Mutex<HashMap<String, MockResponse>>,
    }

    impl MockProvider {
        pub(crate) fn new() -> Self {
            Self {
                responses: Mutex::new(HashMap::new()),
            }
        }

        pub(crate) fn set(&self, path: &str, response: MockResponse) {
            self.responses
                .lock()
                .expect("mock provider lock poisoned")
                .insert(path.into(), response);
        }
    }

    impl LanguageProvider for MockProvider {
        fn extract(
            &self,
            _content: &str,
            path: &str,
            _ctx: &ExtractContext<'_>,
        ) -> Result<ExtractedFile, ExtractError> {
            let guard = self.responses.lock().expect("mock provider lock poisoned");
            match guard.get(path) {
                Some(MockResponse::Ok(extracted)) => Ok(extracted.clone()),
                Some(MockResponse::ParseError(detail)) => Err(ExtractError::Parse {
                    file: path.into(),
                    detail: detail.clone(),
                }),
                None => Err(ExtractError::UnsupportedLanguage { file: path.into() }),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use standardoc_ir::{Blake3Hash, Language, SourceOrigin};

    use super::mock::{MockProvider, MockResponse};
    use super::*;

    fn empty_extracted(path: &str) -> ExtractedFile {
        ExtractedFile {
            file: path.into(),
            language: Language::Rust,
            source_origin: SourceOrigin::Workspace,
            is_external: false,
            content_hash: Blake3Hash::default(),
            byte_size: 0,
            symbols: vec![],
            edges: vec![],
            call_sites: vec![],
            documents: vec![],
            ffi_bindings: vec![],
            module_lookup: None,
        }
    }

    #[test]
    fn mock_returns_registered_extract() {
        let provider = MockProvider::new();
        provider.set(
            "src/lib.rs",
            MockResponse::Ok(empty_extracted("src/lib.rs")),
        );
        let root = PathBuf::from("/ws");
        let ctx = ExtractContext {
            workspace_root: &root,
        };
        let out = provider.extract("", "src/lib.rs", &ctx).unwrap();
        assert_eq!(out.file, "src/lib.rs");
    }

    #[test]
    fn mock_unknown_path_is_unsupported() {
        let provider = MockProvider::new();
        let root = PathBuf::from("/ws");
        let ctx = ExtractContext {
            workspace_root: &root,
        };
        let err = provider.extract("", "nope.rs", &ctx).unwrap_err();
        assert!(matches!(err, ExtractError::UnsupportedLanguage { .. }));
    }

    #[test]
    fn mock_parse_error_is_routed() {
        let provider = MockProvider::new();
        provider.set(
            "src/bad.rs",
            MockResponse::ParseError("unexpected token".into()),
        );
        let root = PathBuf::from("/ws");
        let ctx = ExtractContext {
            workspace_root: &root,
        };
        let err = provider.extract("", "src/bad.rs", &ctx).unwrap_err();
        assert!(matches!(err, ExtractError::Parse { .. }));
    }
}

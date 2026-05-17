use serde::{Deserialize, Serialize};

use crate::call_site::RawCallSite;
use crate::document::RawDocument;
use crate::edge::RawEdge;
use crate::ffi::RawFfiBinding;
use crate::hash::Blake3Hash;
use crate::kinds::{Language, SourceOrigin};
use crate::lookup::ModuleLookup;
use crate::symbol::RawSymbol;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtractedFile {
    pub file: String,
    pub language: Language,
    pub source_origin: SourceOrigin,
    pub is_external: bool,
    pub content_hash: Blake3Hash,
    pub byte_size: u64,
    #[serde(default)]
    pub symbols: Vec<RawSymbol>,
    #[serde(default)]
    pub edges: Vec<RawEdge>,
    #[serde(default)]
    pub call_sites: Vec<RawCallSite>,
    #[serde(default)]
    pub documents: Vec<RawDocument>,
    /// Stage 2 — FFI bindings emitted by language-specific taggers
    /// (`extern "C"` blocks in Rust, non-`static` fns in C, Bun/Deno
    /// `dlopen` in TS, etc.). Persisted into `symbol_ffi_binding` and
    /// consumed by the cross-language resolve pass. Default `vec![]`
    /// keeps older serialized payloads round-trippable.
    #[serde(default)]
    pub ffi_bindings: Vec<RawFfiBinding>,
    /// Stage 3 final-mile (R1) — AOT identifier-resolution table built
    /// during the walk. `Some` for languages that ship an AOT lookup
    /// (TS, Rust). The pipeline persists it via
    /// `put_module_lookup(workspace_id, &lookup)` so cross-workspace
    /// queries can resolve imports against this workspace's modules.
    /// `None` for languages without an AOT pass (Lua, C). Default `None`
    /// keeps older serialized payloads round-trippable.
    #[serde(default)]
    pub module_lookup: Option<ModuleLookup>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_empty() {
        let f = ExtractedFile {
            file: "src/lib.rs".into(),
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
        };
        let json = serde_json::to_string(&f).unwrap();
        let back: ExtractedFile = serde_json::from_str(&json).unwrap();
        assert_eq!(f, back);
    }

    #[test]
    fn round_trip_external_dts() {
        let f = ExtractedFile {
            file: "node_modules/@types/react/index.d.ts".into(),
            language: Language::TypeScript,
            source_origin: SourceOrigin::NodeModulesDts,
            is_external: true,
            content_hash: Blake3Hash::new([0x77; 32]),
            byte_size: 12345,
            symbols: vec![],
            edges: vec![],
            call_sites: vec![],
            documents: vec![],
            ffi_bindings: vec![],
        };
        let json = serde_json::to_string(&f).unwrap();
        let back: ExtractedFile = serde_json::from_str(&json).unwrap();
        assert_eq!(f, back);
    }
}

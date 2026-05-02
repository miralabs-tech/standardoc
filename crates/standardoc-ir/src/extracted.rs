use serde::{Deserialize, Serialize};

use crate::call_site::RawCallSite;
use crate::document::RawDocument;
use crate::edge::RawEdge;
use crate::hash::Blake3Hash;
use crate::kinds::{Language, SourceOrigin};
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
        };
        let json = serde_json::to_string(&f).unwrap();
        let back: ExtractedFile = serde_json::from_str(&json).unwrap();
        assert_eq!(f, back);
    }
}

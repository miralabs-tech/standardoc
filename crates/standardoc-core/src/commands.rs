use standardoc_ir::{ExtractedFile, Language};

#[derive(Debug)]
pub enum IngestCommand {
    UpsertFile {
        path: String,
        extracted: ExtractedFile,
    },
    DeleteFile {
        path: String,
    },
    RecordParseError {
        path: String,
        language: Language,
        detail: String,
    },
    /// Peer counterpart of [`Self::UpsertFile`] — `path` is the raw rel
    /// path inside the peer root (NOT yet scoped); `extracted.file` and
    /// its embedded locations are still unscoped. The writer applies
    /// `peer_path` / `scope_extracted_paths` at dispatch so the storage
    /// boundary is the single scoping site (mirrors L3b's centralized
    /// pattern in `peer_extract::extract_peer_workspace`).
    UpsertPeerFile {
        workspace_id: String,
        path: String,
        extracted: ExtractedFile,
    },
    DeletePeerFile {
        workspace_id: String,
        path: String,
    },
    RecordPeerParseError {
        workspace_id: String,
        path: String,
        language: Language,
        detail: String,
    },
    RescanFromScratch,
}

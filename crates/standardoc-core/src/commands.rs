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
    RescanFromScratch,
}

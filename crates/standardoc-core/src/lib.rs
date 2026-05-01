mod commands;
mod pipeline;
pub mod query;
mod storage;

pub use commands::IngestCommand;
pub use pipeline::{
    ColdStartError, ExtractContext, ExtractError, GitignoreStack, LanguageProvider,
    STDIGNORE_FILENAME, ScanFilters, WatcherError, WatcherHandle, cold_start,
    ensure_stdignore_seed_at, spawn_watcher,
};
pub use storage::error::StorageError;
pub use storage::handle::IndexHandle;

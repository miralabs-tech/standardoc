mod batch;
pub mod cold_start;
mod diff;
mod filters;
mod paths;
mod provider;
mod reindex;
mod watcher;
mod writer;

pub use cold_start::ColdStartError;
pub use filters::{GitignoreStack, STDIGNORE_FILENAME, ScanFilters, ensure_stdignore_seed_at};
pub use provider::{ExtractContext, ExtractError, LanguageProvider};
pub use watcher::{WatcherError, WatcherHandle, spawn_watcher};
pub(crate) use writer::{WriterContext, writer_loop};

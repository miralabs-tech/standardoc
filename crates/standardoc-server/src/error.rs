use standardoc_core::sessions::memory_sync::MemorySyncError;
use standardoc_core::{ColdStartError, SessionsError, StorageError, WatcherError};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ServerError {
    #[error("storage: {0}")]
    Storage(#[from] StorageError),

    #[error("cold start: {0}")]
    ColdStart(#[from] ColdStartError),

    #[error("watcher: {0}")]
    Watcher(#[from] WatcherError),

    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("sessions: {0}")]
    Sessions(#[from] SessionsError),

    #[error("memory sync: {0}")]
    MemorySync(#[from] MemorySyncError),
}

mod batch;
mod c_join;
pub mod cold_start;
pub(crate) mod cross_workspace_post;
mod diff;
pub mod external_invalidation;
mod ffi_resolve;
mod filters;
pub(crate) mod manifest_invalidation;
mod paths;
pub(crate) mod peer_extract;
pub(crate) mod peer_import;
pub(crate) mod projects;
pub(crate) mod provider;
mod reindex;
mod resolve_unresolved;
mod seed_builtins;
mod watcher;
mod writer;

pub use cold_start::ColdStartError;
pub use external_invalidation::{
    LockfileHashes, NpmLockfileKind, compute_lockfile_hashes, handle_lockfile_change,
    invalidate_changed_lockfiles, purge_externals_by_origin, read_stored_hashes,
    tracked_lockfile_paths, write_stored_hashes,
};
pub use filters::{
    GitignoreStack, PATTERN_PREVIEW_WALK_CAP, PatternPreview, PatternPreviewError,
    STDIGNORE_FILENAME, ScanFilters, preview_pattern_matches,
};
pub use provider::{ExtractContext, ExtractError, LanguageProvider};
pub use watcher::{WatcherError, WatcherHandle, spawn_watcher};
pub(crate) use writer::{WriterContext, writer_loop};

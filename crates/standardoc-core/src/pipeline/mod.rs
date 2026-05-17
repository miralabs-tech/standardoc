mod batch;
pub mod cold_start;
mod diff;
pub mod external_invalidation;
mod filters;
pub mod manifest_invalidation;
mod paths;
pub mod peer_extract;
pub mod peer_import;
pub mod projects;
mod provider;
mod reindex;
mod seed_builtins;
mod watcher;
mod writer;

pub use cold_start::ColdStartError;
pub use external_invalidation::{
    LockfileHashes, NpmLockfileKind, compute_lockfile_hashes, handle_lockfile_change,
    invalidate_changed_lockfiles, purge_externals_by_origin, read_stored_hashes,
    tracked_lockfile_paths, write_stored_hashes,
};
pub use manifest_invalidation::{
    MANIFEST_EXTENSIONS, MANIFEST_FILENAMES, handle_manifest_change, is_manifest_file,
};
pub use filters::{
    GitignoreStack, PATTERN_PREVIEW_WALK_CAP, PatternPreview, PatternPreviewError,
    STDIGNORE_FILENAME, ScanFilters, ensure_stdignore_seed_at, preview_pattern_matches,
};
pub use provider::{ExtractContext, ExtractError, LanguageProvider};
pub use watcher::{WatcherError, WatcherHandle, spawn_watcher};
pub(crate) use writer::{WriterContext, writer_loop};

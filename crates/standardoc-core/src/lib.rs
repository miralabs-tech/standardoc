mod commands;
mod cross_workspace_resolver;
pub mod externals;
mod pipeline;
pub mod query;
pub mod sessions;
mod storage;

pub use commands::IngestCommand;
pub use cross_workspace_resolver::DbCrossWorkspaceResolver;
pub use externals::{
    BinaryAvailability, ENV_CARGO_PATH, ENV_LUAROCKS_PATH, ENV_NODE_PATH, ExternalsError,
    ResolveOutcome, Resolver, ResolverRegistry,
};
pub use pipeline::{
    ColdStartError, ExtractContext, ExtractError, GitignoreStack, LanguageProvider, LockfileHashes,
    NpmLockfileKind, PATTERN_PREVIEW_WALK_CAP, PatternPreview, PatternPreviewError,
    STDIGNORE_FILENAME, ScanFilters, WatcherError, WatcherHandle, cold_start,
    compute_lockfile_hashes, ensure_stdignore_seed_at, external_invalidation,
    handle_lockfile_change, invalidate_changed_lockfiles, preview_pattern_matches,
    purge_externals_by_origin, read_stored_hashes, spawn_watcher, tracked_lockfile_paths,
    write_stored_hashes,
};
pub use sessions::{
    SessionRow, SessionStatus, SessionsError, SessionsHandle, UsagePeriod, UsageStatsRow,
};
pub use storage::error::StorageError;
pub use storage::handle::IndexHandle;
pub use storage::migrate::SUPPORTED_SCHEMA_VERSION;

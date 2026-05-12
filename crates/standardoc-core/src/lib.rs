mod commands;
pub mod externals;
mod pipeline;
pub mod query;
pub mod rag;
pub mod sessions;
mod storage;

pub use commands::IngestCommand;
pub use externals::{
    BinaryAvailability, ENV_CARGO_PATH, ENV_LUAROCKS_PATH, ENV_NODE_PATH, ExternalsError,
    ResolveOutcome, Resolver, ResolverRegistry,
};
pub use pipeline::{
    ColdStartError, ExtractContext, ExtractError, GitignoreStack, LanguageProvider, LockfileHashes,
    NpmLockfileKind, STDIGNORE_FILENAME, ScanFilters, WatcherError, WatcherHandle, cold_start,
    compute_lockfile_hashes, ensure_stdignore_seed_at, external_invalidation,
    handle_lockfile_change, invalidate_changed_lockfiles, purge_externals_by_origin,
    read_stored_hashes, spawn_watcher, tracked_lockfile_paths, write_stored_hashes,
};
pub use rag::{
    CoreSymbolLookup, FrontmatterDirective, RagPipeline, RagPipelineError, RagWatcherHandle,
    WORKSPACE_FQDN_LIMIT, discover_prose_sources, is_convention_path, read_frontmatter_directive,
    spawn_rag_watcher,
};
pub use sessions::{
    SessionRow, SessionStatus, SessionsError, SessionsHandle, dump_sessions_to_markdown,
};
pub use storage::error::StorageError;
pub use storage::handle::IndexHandle;
pub use storage::migrate::SUPPORTED_SCHEMA_VERSION;

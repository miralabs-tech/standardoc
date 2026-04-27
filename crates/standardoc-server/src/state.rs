//! Shared workspace state held by every transport (MCP, LSP, SSE).
//!
//! Concurrence:
//! - The index (blocks + collisions + `error_count`) lives behind an
//!   `Arc<RwLock<_>>` so the watcher worker thread can mutate it while MCP
//!   handlers read without blocking.
//! - Revision is an `AtomicU64` outside the lock — clients can read it
//!   lock-free to know whether they should re-fetch the index.
//! - Pause state (`watch_paused`) is an `AtomicBool` checked by worker
//!   before each rescan. MCP `set_watch_paused` tools mutate it.

use standardoc_core::config::{Config, TagSchema};
use standardoc_core::dsl::merged_schemas;
use standardoc_core::model::{DocBlock, IncomingRef};
use standardoc_core::pages::DocPage;
use standardoc_core::pipeline::{scan_and_extract, KeyCollision};
use standardoc_core::scanner::Registry;
use standardoc_core::watcher::Watcher;
use standardoc_web::state::IndexEvent;
use std::io::Stdout;
use std::sync::Mutex;
use std::time::Duration;
use tokio::sync::broadcast;

/// Capacity of the broadcast channel for SSE events. If a slow client
/// overflows the buffer, events are dropped — no back-pressure on the
/// worker. The client catches up using the `revision` on the next heartbeat.
const EVENT_CHANNEL_CAPACITY: usize = 64;

/// Alias for stdout shared between the MCP dispatcher and worker. Every
/// write goes through a mutex -> no interleaved JSON-RPC lines.
pub(crate) type SharedStdout = Arc<Mutex<Stdout>>;
use standardoc_core::lang::LanguageProvider;
use standardoc_lang_python::PythonProvider;
use standardoc_lang_rust::RustProvider;
use standardoc_lang_tree_sitter::{LanguageFn, TreeSitterProvider};
use standardoc_lang_ts::TsProvider;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

/// Bundle of mutable index-side data. Guarded by a single lock so updates
/// (full or incremental rescan) stay atomic: clients always see a coherent
/// state, never a half-updated one.
pub(crate) struct IndexState {
    pub(crate) blocks: BTreeMap<String, DocBlock>,
    /// Narrative pages loaded from `.standardoc/pages/`. User-curated source
    /// of truth, updated by the watcher alongside blocks.
    pub(crate) pages: BTreeMap<String, DocPage>,
    pub(crate) collisions: Vec<KeyCollision>,
    pub(crate) error_count: usize,
    /// Source locations tracked by key — allows recomputing collisions on
    /// each incremental rescan without a full scan. A key with multiple
    /// entries means a collision.
    pub(crate) key_locations: BTreeMap<String, Vec<standardoc_core::pipeline::PathLine>>,
    /// Reverse cross-reference index: for each symbol **short name**
    /// (label / last FQN segment), the list of blocks that reference it.
    /// Maintained in parallel with `blocks` to answer tools quickly:
    /// `find_usages` / `find_implementations` / `search_by_*`.
    ///
    /// We index by short name to avoid relying on perfect FQN resolution
    /// (which would require `use`/`import` analysis). Trade-off:
    /// `find_usages("ParseError")` can return matches for different
    /// `ParseError` symbols — the agent can filter by path if needed.
    pub(crate) incoming: BTreeMap<String, Vec<IncomingRef>>,
}

pub(crate) struct ServerState {
    workspace_root: PathBuf,
    config: Config,
    schemas: BTreeMap<String, TagSchema>,
    index: Arc<RwLock<IndexState>>,
    revision: Arc<AtomicU64>,
    /// Runtime flag that temporarily freezes the worker. MCP
    /// `set_watch_paused` mutates it. Events are still received from the
    /// watcher during pause but are drained without rescanning.
    watch_paused: Arc<AtomicBool>,
    /// Maps file extension (e.g. ".lua") to a tree-sitter language function.
    /// Populated at boot from all registered tree-sitter providers (built-in
    /// and dynamic). Used by the `get_comments` MCP tool.
    ts_comment_langs: HashMap<String, LanguageFn>,
    /// Broadcast sender for SSE events (web mode). Always allocated:
    /// if nobody subscribes, `send` returns `Ok(0)` with near-zero cost.
    events: broadcast::Sender<IndexEvent>,
    /// Handle to the watcher + worker thread. Drop = clean shutdown.
    watcher_runtime: Option<WatcherRuntime>,
}

/// Holds the watcher and its worker thread. `Drop` triggers shutdown by
/// closing the worker-side receiver — the thread exits its loop and joins.
struct WatcherRuntime {
    _watcher: Watcher,
    worker: Option<std::thread::JoinHandle<()>>,
    shutdown: Arc<AtomicBool>,
}

impl Drop for WatcherRuntime {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        if let Some(handle) = self.worker.take() {
            // Join waits at most a few seconds — worker checks shutdown
            // with a short recv_timeout interval.
            let _ = handle.join();
        }
    }
}

impl ServerState {
    pub(crate) fn boot(workspace: &Path, stdout: &SharedStdout) -> Result<Self, std::io::Error> {
        Self::boot_with_options(workspace, Some(stdout))
    }

    /// Boot variant for transports that do not use JSON-RPC stdout
    /// (typically web mode). Worker then only pushes to the broadcast
    /// channel, so stdout stays clean.
    pub(crate) fn boot_for_web(workspace: &Path) -> Result<Self, std::io::Error> {
        Self::boot_with_options(workspace, None)
    }

    fn boot_with_options(
        workspace: &Path,
        stdout: Option<&SharedStdout>,
    ) -> Result<Self, std::io::Error> {
        // Load workspace `.standardoc.json` if present, otherwise defaults.
        // Best effort: malformed files must not block boot; error is logged
        // to stderr (see `load_from_workspace_or_default`).
        let config = Config::load_from_workspace_or_default(workspace);

        let registry = build_registry_with_workspace(Some(workspace));
        let ts_comment_langs = build_ts_comment_langs(Some(workspace));
        let mut report = scan_and_extract(workspace, &registry, &config)?;
        let workspace_root = report.workspace_root.clone();
        apply_ts_public_surface(&mut report.blocks, &workspace_root);
        warn_legacy_reference_pages(&report.pages);

        let schemas = merged_schemas(&config.tags);
        let key_locations = build_key_locations(&report.blocks);
        let incoming = build_incoming_index(&report.blocks);
        let index = Arc::new(RwLock::new(IndexState {
            blocks: report.blocks,
            pages: report.pages,
            collisions: report.collisions,
            error_count: report.errors.len(),
            key_locations,
            incoming,
        }));
        let revision = Arc::new(AtomicU64::new(0));
        let watch_paused = Arc::new(AtomicBool::new(false));
        let (events, _) = broadcast::channel::<IndexEvent>(EVENT_CHANNEL_CAPACITY);

        // Attempt to start watcher. If it fails (e.g. workspace permissions),
        // log and continue without it — server stays functional but live
        // updates are disabled.
        //
        // `config.watch.enabled = false` allows explicit opt-out:
        // useful in CI for a frozen index, or while debugging in
        // "manual rescan only" mode.
        let watcher_runtime = if config.watch.enabled {
            let debounce = Duration::from_millis(config.watch.debounce_ms);
            match Watcher::start_with_debounce(&workspace_root, debounce) {
                Ok((watcher, rx)) => {
                    let shutdown = Arc::new(AtomicBool::new(false));
                    let worker = crate::worker::spawn(crate::worker::WorkerConfig {
                        workspace_root: workspace_root.clone(),
                        index: Arc::clone(&index),
                        revision: Arc::clone(&revision),
                        watch_paused: Arc::clone(&watch_paused),
                        shutdown: Arc::clone(&shutdown),
                        rx,
                        auto_pause_parse_errors: config.watch.auto_pause_parse_errors,
                        auto_pause_window: Duration::from_millis(config.watch.auto_pause_window_ms),
                        stdout: stdout.map(Arc::clone),
                        events: events.clone(),
                    });
                    Some(WatcherRuntime {
                        _watcher: watcher,
                        worker: Some(worker),
                        shutdown,
                    })
                }
                Err(err) => {
                    eprintln!("watcher: failed to start, live updates disabled: {err}");
                    None
                }
            }
        } else {
            None
        };

        Ok(Self {
            workspace_root,
            config,
            schemas,
            index,
            revision,
            watch_paused,
            ts_comment_langs,
            events,
            watcher_runtime,
        })
    }

    pub(crate) fn subscribe_events(&self) -> broadcast::Receiver<IndexEvent> {
        self.events.subscribe()
    }

    /// Rescan only narrative pages without touching blocks.
    /// Used after API-side mutations (PUT/DELETE /api/page) so subsequent
    /// reads see fresh state without waiting for watcher cycle. Also bumps
    /// revision and emits an SSE event so connected clients invalidate cache.
    pub(crate) fn rescan_pages_now(&self) {
        let new_pages = standardoc_core::pages::scan_pages(self.workspace_root());
        {
            let mut guard = self
                .index
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard.pages = new_pages;
        }
        let rev = self.revision.fetch_add(1, Ordering::AcqRel) + 1;
        let _ = self.events.send(IndexEvent::IndexChanged { revision: rev });
    }

    pub(crate) fn rescan(&self) -> Result<usize, std::io::Error> {
        let registry = build_registry_with_workspace(Some(&self.workspace_root));
        let mut report = scan_and_extract(&self.workspace_root, &registry, &self.config)?;
        apply_ts_public_surface(&mut report.blocks, &self.workspace_root);
        let count = report.blocks.len();
        let key_locations = build_key_locations(&report.blocks);
        let incoming = build_incoming_index(&report.blocks);

        let new_state = IndexState {
            blocks: report.blocks,
            pages: report.pages,
            collisions: report.collisions,
            error_count: report.errors.len(),
            key_locations,
            incoming,
        };

        {
            let mut guard = self
                .index
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *guard = new_state;
        }
        self.revision.fetch_add(1, Ordering::Release);
        Ok(count)
    }

    pub(crate) fn index(&self) -> std::sync::RwLockReadGuard<'_, IndexState> {
        self.index
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(crate) const fn schemas(&self) -> &BTreeMap<String, TagSchema> {
        &self.schemas
    }

    pub(crate) const fn config(&self) -> &Config {
        &self.config
    }

    pub(crate) fn revision(&self) -> u64 {
        self.revision.load(Ordering::Acquire)
    }

    pub(crate) fn watch_paused(&self) -> bool {
        self.watch_paused.load(Ordering::Acquire)
    }

    pub(crate) fn set_watch_paused(&self, paused: bool) {
        self.watch_paused.store(paused, Ordering::Release);
    }

    /// `true` if watcher started successfully at boot, `false` in degraded
    /// mode (no live updates).
    pub(crate) const fn has_watcher(&self) -> bool {
        self.watcher_runtime.is_some()
    }

    pub(crate) fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    pub(crate) fn workspace_root_join(&self, relative: &Path) -> PathBuf {
        self.workspace_root.join(relative)
    }

    /// Look up the tree-sitter language function for a file extension (e.g. `".lua"`).
    /// Returns `None` if no tree-sitter grammar is registered for that extension.
    pub(crate) fn ts_language_for_extension(&self, ext: &str) -> Option<LanguageFn> {
        self.ts_comment_langs.get(ext).copied()
    }
}

/// Builds the extension → tree-sitter language function map used by `get_comments`.
/// Only tree-sitter-backed providers are included (Rust/TS/Python use their own parsers).
fn build_ts_comment_langs(workspace: Option<&Path>) -> HashMap<String, LanguageFn> {
    let mut map = HashMap::new();

    let lua = TreeSitterProvider::lua();
    for ext in lua.extensions() {
        map.insert((*ext).to_string(), lua.language_fn());
    }

    if let Some(ws) = workspace {
        for def in standardoc_core::lang_def::load_workspace_languages(ws) {
            if let Ok(provider) = TreeSitterProvider::from_lang_def(&def) {
                let lang_fn = provider.language_fn();
                for ext in provider.extensions() {
                    map.entry((*ext).to_string()).or_insert(lang_fn);
                }
            }
        }
    }

    map
}

/// Also loads the workspace's `.standardoc/languages/*.json` files. Used
/// at boot — dynamic providers register **after** the built-ins, so on
/// extension conflict the built-in wins. (If the user wants to override
/// Lua, they'd change the extension in their JSON, or wait for a future
/// override-list mechanism.)
pub(crate) fn build_registry_with_workspace(workspace: Option<&Path>) -> Registry {
    let mut builder = Registry::builder()
        .with(RustProvider)
        .with(TsProvider)
        .with(PythonProvider)
        .with(TreeSitterProvider::lua());

    if let Some(ws) = workspace {
        for def in standardoc_core::lang_def::load_workspace_languages(ws) {
            builder = register_dynamic_provider(builder, &def);
        }
    }

    builder.build()
}

/// Dispatcher: depending on the `LanguageDef`'s `backend`, instantiate
/// the right provider (tree-sitter fork or regex) and register it.
/// Construction errors are logged to stderr — best-effort, we don't stop
/// the boot for a single malformed provider.
fn register_dynamic_provider(
    builder: standardoc_core::scanner::RegistryBuilder,
    def: &standardoc_core::lang_def::LanguageDef,
) -> standardoc_core::scanner::RegistryBuilder {
    use standardoc_core::lang_def::LanguageBackend;
    use standardoc_core::lang_regex::RegexProvider;
    match &def.backend {
        LanguageBackend::TreeSitterFork { base, .. } => {
            match TreeSitterProvider::from_lang_def(def) {
                Ok(provider) => {
                    eprintln!(
                        "standardoc: loaded tree-sitter fork '{}' (extends {base})",
                        def.id
                    );
                    builder.with(provider)
                }
                Err(err) => {
                    eprintln!("standardoc: skipping language def '{}': {err}", def.id);
                    builder
                }
            }
        }
        LanguageBackend::Regex { .. } => match RegexProvider::from_lang_def(def) {
            Ok(provider) => {
                eprintln!("standardoc: loaded regex provider '{}'", def.id);
                builder.with(provider)
            }
            Err(err) => {
                eprintln!("standardoc: skipping language def '{}': {err}", def.id);
                builder
            }
        },
    }
}

/// Filter TS blocks to only include those in the public surface of the workspace
/// entry point (`src/index.ts` or `index.ts`). Non-TS blocks are unaffected.
fn apply_ts_public_surface(blocks: &mut BTreeMap<String, DocBlock>, workspace: &Path) {
    let candidates = [
        workspace.join("src/index.ts"),
        workspace.join("src/index.js"),
        workspace.join("index.ts"),
        workspace.join("index.js"),
    ];
    let Some(entry_path) = candidates.iter().find(|p| p.exists()) else {
        return;
    };
    let surface = standardoc_lang_ts::build_public_surface(entry_path);
    if surface.is_empty() {
        return;
    }

    blocks.retain(|_, block| {
        let ext = block
            .meta
            .path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");
        if !matches!(ext, "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs") {
            return true;
        }
        let abs = if block.meta.path.is_absolute() {
            block.meta.path.clone()
        } else {
            workspace.join(&block.meta.path)
        };
        let Ok(canonical) = abs.canonicalize() else {
            return true;
        };
        match surface.get(&canonical) {
            None => false,
            Some(None) => true,
            // Named export: accept the block if its own name OR any ancestor segment
            // matches an exported name (handles interface member sub-blocks like
            // `Matcher.with` when `Matcher` is the exported name).
            Some(Some(names)) => block.key.as_str().split('.').any(|seg| names.contains(seg)),
        }
    });
}

/// Rebuild `key_locations` tracker from a full blocks state.
/// Used at boot and after each full rescan — incremental rescans maintain
/// this tracker as they go, without calling this function.
pub(crate) fn build_key_locations(
    blocks: &BTreeMap<String, DocBlock>,
) -> BTreeMap<String, Vec<standardoc_core::pipeline::PathLine>> {
    let mut map: BTreeMap<String, Vec<standardoc_core::pipeline::PathLine>> = BTreeMap::new();
    for (key, block) in blocks {
        map.entry(key.clone())
            .or_default()
            .push(standardoc_core::pipeline::PathLine {
                path: block.meta.path.clone(),
                line: block.meta.line_start,
            });
    }
    map
}

/// Build reverse index `target_short_name -> [referrers]`. A symbol whose
/// `outgoing.target` is `Foo` appears in `incoming["Foo"]` as an
/// `IncomingRef` pointing to the referrer symbol.
pub(crate) fn build_incoming_index(
    blocks: &BTreeMap<String, DocBlock>,
) -> BTreeMap<String, Vec<IncomingRef>> {
    let mut map: BTreeMap<String, Vec<IncomingRef>> = BTreeMap::new();
    for (referrer_key, block) in blocks {
        let Some(symbol) = &block.symbol else {
            continue;
        };
        for sref in &symbol.references.outgoing {
            map.entry(sref.target.clone())
                .or_default()
                .push(IncomingRef {
                    from_key: referrer_key.clone(),
                    kind: sref.kind,
                    line: sref.line,
                });
        }
    }
    map
}

/// Phase 3 migration warning: detect legacy `reference/<key>.md` shadow files
/// left over from Phase 2 when those slugs were editable. Reference pages are
/// now strictly read-only — these files are still served to avoid silent data
/// loss, but the user should move their content into a guide page that embeds
/// the reference via `<Reference of="..." />`.
fn warn_legacy_reference_pages(pages: &BTreeMap<String, DocPage>) {
    for slug in pages.keys() {
        if slug == "reference" || slug.starts_with("reference/") {
            eprintln!(
                "standardoc: legacy shadow page found at slug \"{slug}\" — \
                 reference/* is reserved in Phase 3. Move your content to a \
                 guide page (e.g. \"guides/<name>\") that embeds the reference \
                 via <Reference of=\"...\" />."
            );
        }
    }
}

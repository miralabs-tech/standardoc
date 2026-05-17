//! Cross-workspace symbol resolution boundary (Stage 3 R3).
//!
//! The visitor needs to know whether a candidate
//! `(origin_module, origin_symbol)` pair binds in a peer workspace's
//! persisted `ModuleLookup`. The trait is defined here (IR layer) so
//! the lang-provider crate can depend on it without pulling in the
//! storage backend. The DB-backed implementation
//! (`DbCrossWorkspaceResolver`) lives in `standardoc-core`.
//!
//! The three outcomes ([`CrossWorkspaceLookup::Hit`],
//! [`CrossWorkspaceLookup::KnownPeerMissingSymbol`],
//! [`CrossWorkspaceLookup::Unknown`]) let the visitor distinguish:
//!
//! - **Hit**: emit `Resolved { fqdn }` and stamp `cross-workspace` /
//!   `peer-<ws_id>` attributes on the edge.
//! - **KnownPeerMissingSymbol**: emit
//!   `UnresolvedBridge { bridge: BridgeKind("custom:cross-workspace"),
//!   name }` so the bridge carries typed semantics rather than getting
//!   lost in the generic `Unresolved` bucket.
//! - **Unknown**: the candidate is not a cross-workspace reference;
//!   caller falls through to its normal `Unresolved` path.

/// Resolver boundary for the visitor. Implementations are expected to
/// be cheap-on-cache-hit (the visitor calls this once per identifier
/// per file) and `Send + Sync` for parallel extract passes.
pub trait CrossWorkspaceResolver: Send + Sync {
    fn resolve(&self, origin_module: &str, origin_symbol: &str) -> CrossWorkspaceLookup;
}

/// Outcome of a cross-workspace lookup. See module docs for caller semantics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CrossWorkspaceLookup {
    /// A peer workspace exposes `origin_symbol` at the root scope of
    /// `origin_module`. `fqdn` is the synthesised
    /// `<origin_module>::<origin_symbol>` for downstream edge targets.
    Hit {
        workspace_id: String,
        fqdn: String,
    },
    /// A peer workspace owns `origin_module` but does not expose
    /// `origin_symbol` at the root scope. Caller emits a typed
    /// `UnresolvedBridge` so the gap is visible in the viz payload
    /// rather than aliased to a regular unresolved.
    KnownPeerMissingSymbol { workspace_id: String },
    /// No peer workspace owns `origin_module`. Caller treats the
    /// reference as a normal local-unresolved.
    Unknown,
}

impl CrossWorkspaceLookup {
    /// Slug used for the `BridgeKind` carried by the
    /// `UnresolvedBridge` variant when the visitor emits a miss. Kept
    /// in the IR crate (rather than core) so providers don't import a
    /// magic string from the DB layer.
    pub const BRIDGE_SLUG: &'static str = "custom:cross-workspace";

    /// Convenience: `true` when the variant carries an edge target
    /// (`Hit`). Useful for visitors that fast-path Hit vs Miss vs Unknown.
    #[must_use]
    pub const fn is_hit(&self) -> bool {
        matches!(self, Self::Hit { .. })
    }
}

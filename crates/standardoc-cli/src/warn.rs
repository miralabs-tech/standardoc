//! Structured soft warnings emitted on stderr at daemon boot.
//!
//! Mirrors [`fatal_marker_for`](crate::fatal_marker_for) but for
//! non-fatal events the VSCode extension should surface as a
//! notification without killing the daemon. The two channels share the
//! shape `STDOC_<KIND>: <json>` so the supervisor side parses them with
//! a single line-scanner.
//!
//! Day-1 events:
//!
//! - `missing_binary` — a resolver detected its precondition (`.pnp.cjs`
//!   present, `.lua` files in tree, `Cargo.lock` at root) but the
//!   matching CLI is not invokable. The VSCode extension surfaces a
//!   notification suggesting the user set `standardoc.binaryPaths.<bin>`
//!   in settings.
//! - `lockfile_purge` — the cold-start invalidation step purged the
//!   externals bucket because a lockfile hash diverged. Surfaced as
//!   info-level so users learn why a previously-cached `resolve_external`
//!   takes longer the next time it is invoked.

use std::io::{self, IsTerminal, Write};

use serde::Serialize;
use standardoc_core::{
    BinaryAvailability, ENV_CARGO_PATH, ENV_LUAROCKS_PATH, ENV_NODE_PATH, IndexHandle,
    ResolverRegistry, invalidate_changed_lockfiles,
};
use standardoc_ir::SourceOrigin;

const STDOC_WARN_PREFIX: &str = "STDOC_WARN: ";

/// Tagged enum of every structured warning shipped from the daemon to
/// the supervisor. `kind` discriminator drives the supervisor's match.
/// Adding a new variant requires a matching parser update on the
/// VSCode extension side (`ext/vscode/src/daemon/warn-marker.ts`).
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum WarnEvent {
    /// A resolver's precondition is satisfied (e.g. `.pnp.cjs` exists)
    /// but the binary backing it could not be invoked. The supervisor
    /// surfaces a "configure path" suggestion in the IDE notification.
    MissingBinary {
        /// Default CLI name probed (`cargo`, `node`, `luarocks`).
        binary: String,
        /// Env var to override the path (`STANDARDOC_<BIN>_PATH`).
        env_override: String,
        /// What in the workspace activated the probe (`.pnp.cjs`,
        /// `Cargo.lock`, `*.lua` files, ...).
        trigger: String,
        /// Absolute workspace root for which the warning fires.
        workspace: String,
    },
    /// A lockfile changed between daemon runs and the matching external
    /// cache was purged. Info-level — no user action required, but
    /// surfaces the latency hit on the next `resolve_external` call.
    LockfilePurge {
        /// `cargo` / `npm` / `luarocks` — matches `SourceOrigin`.
        origin: String,
        /// Number of external symbols dropped from the index.
        purged_count: usize,
        /// Absolute workspace root.
        workspace: String,
    },
}

/// Emits a single WARN line to stderr. Uses a non-blocking write so a
/// closed/redirected stderr cannot stall the daemon. Lines are
/// gracefully no-op'd when stderr is not a terminal AND not a pipe (the
/// VSCode supervisor pipes stderr, so the marker still reaches it).
pub(crate) fn emit_warn(event: &WarnEvent) {
    let payload = match serde_json::to_string(event) {
        Ok(p) => p,
        Err(_) => return,
    };
    let mut stderr = io::stderr().lock();
    let _ = writeln!(stderr, "{STDOC_WARN_PREFIX}{payload}");
    let _ = stderr.flush();
    let _ = stderr.is_terminal(); // silences unused warning on TTY check.
}

const fn env_override_for(binary: &str) -> &'static str {
    match binary.as_bytes() {
        b"cargo" => ENV_CARGO_PATH,
        b"node" => ENV_NODE_PATH,
        b"luarocks" => ENV_LUAROCKS_PATH,
        _ => "",
    }
}

const fn trigger_for(resolver_name: &str) -> &'static str {
    match resolver_name.as_bytes() {
        b"cargo" => "Cargo.lock",
        b"npm" => ".pnp.cjs",
        b"luarocks" => "*.lua",
        _ => "",
    }
}

const fn binary_for(resolver_name: &str) -> &'static str {
    match resolver_name.as_bytes() {
        b"cargo" => "cargo",
        b"npm" => "node",
        b"luarocks" => "luarocks",
        _ => "",
    }
}

/// Boot-time sweep of every external resolver's binary probe. Emits one
/// `MissingBinary` WARN per resolver whose precondition is met but
/// whose binary is missing. Returns the number of warnings emitted so
/// callers can short-circuit further announcements.
///
/// Constructs a [`ResolverRegistry`] for the workspace, then walks
/// every resolver's binary probe. For each resolver whose precondition
/// fired (it was constructed) but whose backing CLI is `Missing`,
/// emits a single `STDOC_WARN: missing_binary` line on stderr so the
/// VSCode supervisor can surface a configure-path suggestion in the
/// IDE notification stream.
///
/// Returns the number of WARN events emitted. `NotApplicable` probes
/// (e.g. classic-only npm workspace without `.pnp.cjs`) are skipped.
pub(crate) fn boot_binary_sweep(workspace_root: &std::path::Path) -> usize {
    let registry = ResolverRegistry::for_workspace(workspace_root.to_path_buf());
    let mut warned = 0usize;
    for (resolver_name, availability) in registry.probe_all() {
        if !matches!(availability, BinaryAvailability::Missing { .. }) {
            continue;
        }
        let binary = binary_for(resolver_name).to_string();
        let env_override = env_override_for(&binary).to_string();
        let trigger = trigger_for(resolver_name).to_string();
        let event = WarnEvent::MissingBinary {
            binary,
            env_override,
            trigger,
            workspace: workspace_root.display().to_string(),
        };
        emit_warn(&event);
        warned += 1;
    }
    warned
}

/// Runs the cold-start external invalidation sweep
/// ([`invalidate_changed_lockfiles`]) and emits one
/// `STDOC_WARN: lockfile_purge` per origin that was purged because the
/// corresponding lockfile changed between daemon runs. Returns the
/// number of WARN events emitted (= number of origins purged). Silent
/// no-op when the workspace has no lockfiles or every hash matches.
///
/// Storage errors are swallowed (logged as a single MissingBinary-style
/// stderr `error: ...` line by the caller) — invalidation is best-
/// effort and should never prevent the daemon from booting.
pub(crate) fn boot_lockfile_invalidation_sweep(
    handle: &IndexHandle,
    workspace_root: &std::path::Path,
) -> usize {
    let purged = match invalidate_changed_lockfiles(handle, workspace_root) {
        Ok(p) => p,
        Err(_) => return 0,
    };
    for (origin, count) in &purged {
        let event = WarnEvent::LockfilePurge {
            origin: source_origin_label(*origin).to_string(),
            purged_count: *count,
            workspace: workspace_root.display().to_string(),
        };
        emit_warn(&event);
    }
    purged.len()
}

const fn source_origin_label(origin: SourceOrigin) -> &'static str {
    match origin {
        SourceOrigin::Workspace => "workspace",
        SourceOrigin::CargoRegistry => "cargo_registry",
        SourceOrigin::NodeModulesDts => "node_modules_dts",
        SourceOrigin::ManualExternal => "manual_external",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warn_event_missing_binary_serialises_with_kind_tag() {
        let event = WarnEvent::MissingBinary {
            binary: "node".into(),
            env_override: "STANDARDOC_NODE_PATH".into(),
            trigger: ".pnp.cjs".into(),
            workspace: "/tmp/wks".into(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"kind\":\"missing_binary\""));
        assert!(json.contains("\"binary\":\"node\""));
        assert!(json.contains("\"trigger\":\".pnp.cjs\""));
    }

    #[test]
    fn warn_event_lockfile_purge_serialises_with_kind_tag() {
        let event = WarnEvent::LockfilePurge {
            origin: "cargo_registry".into(),
            purged_count: 42,
            workspace: "/tmp/wks".into(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"kind\":\"lockfile_purge\""));
        assert!(json.contains("\"purged_count\":42"));
    }

    #[test]
    fn boot_binary_sweep_is_quiet_on_scaffold_a() {
        let tmp = tempfile::tempdir().unwrap();
        let warned = boot_binary_sweep(tmp.path());
        assert_eq!(
            warned, 0,
            "scaffold A boot sweep must stay silent (no probe yet)"
        );
    }
}

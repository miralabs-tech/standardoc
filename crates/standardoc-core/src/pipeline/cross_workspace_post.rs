//! Stage 3 R3 — post-extract cross-workspace edge strengthening.
//!
//! After a language provider returns an [`ExtractedFile`], this pass
//! walks the file's edges and consults the [`CrossWorkspaceResolver`]
//! for any target whose leftmost segment lives outside the current
//! workspace's local prefix. Three outcomes:
//!
//! - [`CrossWorkspaceLookup::Hit`]: rewrite `Resolved { fqdn }` to the
//!   peer's fqdn and stamp `cross-workspace` + `peer-<ws_id>` attrs.
//! - [`CrossWorkspaceLookup::KnownPeerMissingSymbol`]: rewrite to
//!   `UnresolvedBridge { bridge: custom:cross-workspace, name }` with
//!   the same attribute pair so the viz can surface "we tried, the
//!   peer doesn't export this".
//! - [`CrossWorkspaceLookup::Unknown`]: leave the edge unchanged.
//!
//! Implemented as a post-process so language walkers stay agnostic of
//! cross-workspace state — the resolver is purely a workspace-level
//! concern. The pass is a no-op for languages where the module_fqdn
//! doesn't carry a single-segment local prefix (Vue, Svelte — both
//! drive their TS body through the TS extractor anyway, so the SFC
//! shell itself stays opaque to cross-workspace resolution).

use standardoc_ir::{
    BridgeKind, CrossWorkspaceLookup, CrossWorkspaceResolver, ExtractedFile, Language,
    ResolvedOrUnresolved,
};

/// Strengthen cross-workspace edges in-place. Idempotent — running it
/// twice on the same file leaves the second pass as a no-op because the
/// rewritten variants no longer match the candidate pattern.
pub(crate) fn rewrite_cross_workspace_edges(
    extracted: &mut ExtractedFile,
    resolver: &dyn CrossWorkspaceResolver,
) {
    let Some(lookup) = &extracted.module_lookup else {
        return;
    };
    if !language_supports_workspace_prefix(lookup.language) {
        return;
    }
    let local_prefix = match lookup.module_fqdn.split_once("::") {
        Some((head, _)) => head,
        None => lookup.module_fqdn.as_str(),
    };
    if local_prefix.is_empty() {
        return;
    }

    for edge in &mut extracted.edges {
        let candidate = match &edge.to {
            ResolvedOrUnresolved::Resolved { fqdn } => fqdn.clone(),
            ResolvedOrUnresolved::Unresolved { name } => name.clone(),
            ResolvedOrUnresolved::UnresolvedBridge { .. } => continue,
        };
        if is_local(&candidate, local_prefix) {
            continue;
        }
        let Some(outcome) = resolve_with_suffix_chain(resolver, &candidate) else {
            continue;
        };
        match outcome {
            CrossWorkspaceLookup::Hit { workspace_id, fqdn } => {
                edge.to = ResolvedOrUnresolved::Resolved { fqdn };
                stamp_attrs(&mut edge.attributes, &workspace_id);
                edge.confidence = edge.to.default_confidence();
            }
            CrossWorkspaceLookup::KnownPeerMissingSymbol { workspace_id } => {
                edge.to = ResolvedOrUnresolved::UnresolvedBridge {
                    bridge: BridgeKind::from(CrossWorkspaceLookup::BRIDGE_SLUG),
                    name: candidate,
                };
                stamp_attrs(&mut edge.attributes, &workspace_id);
                edge.confidence = edge.to.default_confidence();
            }
            CrossWorkspaceLookup::Unknown => {}
        }
    }
}

/// Bug E-2 — find a (module, symbol) split that the resolver can answer
/// and compose the result with any trailing path segments preserved.
///
/// `candidate.rsplit_once("::")` alone fails for the common
/// `<crate>::<re_export>::<method>` pattern :
/// `lur_common::Span::new` → (`lur_common::Span`, `new`) is asked, but
/// `lur_common::Span` isn't a module (it's a re-export symbol), so the
/// resolver returns `Unknown` even though `lur_common` does export
/// `Span` (whose `resolved_fqdn` is the canonical `lur-common::span::Span`).
///
/// Walk split points longest-module-first. Each candidate split is
/// `(prefix, head)` where `head` is one segment and any remaining tail
/// stays as a suffix to append to the resolver's hit FQDN. The first
/// `Hit` wins ; if every split returns `Unknown` we fall back to the
/// first `KnownPeerMissingSymbol` (or `None`).
pub(crate) fn resolve_with_suffix_chain(
    resolver: &dyn CrossWorkspaceResolver,
    candidate: &str,
) -> Option<CrossWorkspaceLookup> {
    let segments: Vec<&str> = candidate.split("::").collect();
    if segments.len() < 2 {
        return None;
    }
    let mut fallback: Option<CrossWorkspaceLookup> = None;
    for split in (1..segments.len()).rev() {
        let prefix = segments[..split].join("::");
        let head = segments[split];
        let tail = if split + 1 < segments.len() {
            Some(segments[split + 1..].join("::"))
        } else {
            None
        };
        match resolver.resolve(&prefix, head) {
            CrossWorkspaceLookup::Hit { workspace_id, fqdn } => {
                let final_fqdn = match &tail {
                    Some(t) => format!("{fqdn}::{t}"),
                    None => fqdn,
                };
                return Some(CrossWorkspaceLookup::Hit {
                    workspace_id,
                    fqdn: final_fqdn,
                });
            }
            CrossWorkspaceLookup::KnownPeerMissingSymbol { workspace_id } => {
                if fallback.is_none() {
                    fallback = Some(CrossWorkspaceLookup::KnownPeerMissingSymbol { workspace_id });
                }
            }
            CrossWorkspaceLookup::Unknown => {}
        }
    }
    fallback
}

const fn language_supports_workspace_prefix(lang: Language) -> bool {
    matches!(
        lang,
        Language::Rust | Language::TypeScript | Language::JavaScript | Language::C | Language::Lua
    )
}

fn is_local(fqdn: &str, local_prefix: &str) -> bool {
    if fqdn == local_prefix {
        return true;
    }
    let Some(rest) = fqdn.strip_prefix(local_prefix) else {
        return false;
    };
    rest.starts_with("::")
}

fn stamp_attrs(attrs: &mut Vec<String>, workspace_id: &str) {
    if !attrs.iter().any(|a| a == "cross-workspace") {
        attrs.push("cross-workspace".to_string());
    }
    let peer_attr = format!("peer-{workspace_id}");
    if !attrs.iter().any(|a| a == &peer_attr) {
        attrs.push(peer_attr);
    }
}

#[cfg(test)]
mod tests;

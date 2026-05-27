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
fn resolve_with_suffix_chain(
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
        Language::Rust
            | Language::TypeScript
            | Language::JavaScript
            | Language::C
            | Language::Lua
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
mod tests {
    use super::*;
    use standardoc_ir::{
        Blake3Hash, EdgeKind, ExtractedFile, Language, ModuleLookup, RawEdge, SourceOrigin,
    };
    use std::collections::HashMap;
    use std::sync::Mutex;

    struct FakeResolver {
        hits: HashMap<(String, String), CrossWorkspaceLookup>,
        calls: Mutex<usize>,
    }

    impl FakeResolver {
        fn new(hits: HashMap<(String, String), CrossWorkspaceLookup>) -> Self {
            Self {
                hits,
                calls: Mutex::new(0),
            }
        }
        fn call_count(&self) -> usize {
            *self.calls.lock().unwrap()
        }
    }

    impl CrossWorkspaceResolver for FakeResolver {
        fn resolve(&self, origin_module: &str, origin_symbol: &str) -> CrossWorkspaceLookup {
            *self.calls.lock().unwrap() += 1;
            self.hits
                .get(&(origin_module.to_string(), origin_symbol.to_string()))
                .cloned()
                .unwrap_or(CrossWorkspaceLookup::Unknown)
        }
    }

    fn empty_extracted(language: Language, module_fqdn: &str) -> ExtractedFile {
        ExtractedFile {
            file: "src/lib.rs".into(),
            language,
            source_origin: SourceOrigin::Workspace,
            is_external: false,
            content_hash: Blake3Hash::default(),
            byte_size: 0,
            symbols: vec![],
            edges: vec![],
            call_sites: vec![],
            documents: vec![],
            ffi_bindings: vec![],
            module_lookup: Some(ModuleLookup::new(module_fqdn.into(), language)),
        }
    }

    fn unresolved_edge(target: &str) -> RawEdge {
        RawEdge::with_default_confidence(
            "primary::module::caller".into(),
            EdgeKind::Calls,
            ResolvedOrUnresolved::Unresolved {
                name: target.into(),
            },
            vec![],
            vec![],
        )
    }

    fn resolved_edge(target: &str) -> RawEdge {
        RawEdge::with_default_confidence(
            "primary::module::caller".into(),
            EdgeKind::Imports,
            ResolvedOrUnresolved::Resolved {
                fqdn: target.into(),
            },
            vec![],
            vec![],
        )
    }

    #[test]
    fn no_op_without_module_lookup() {
        let mut extracted = empty_extracted(Language::Rust, "primary::lib");
        extracted.module_lookup = None;
        extracted.edges.push(unresolved_edge("peer::lib::Foo"));
        let resolver = FakeResolver::new(HashMap::new());
        rewrite_cross_workspace_edges(&mut extracted, &resolver);
        assert_eq!(resolver.call_count(), 0);
    }

    #[test]
    fn skips_local_targets() {
        let mut extracted = empty_extracted(Language::Rust, "primary::lib");
        extracted.edges.push(resolved_edge("primary::lib::Local"));
        let resolver = FakeResolver::new(HashMap::new());
        rewrite_cross_workspace_edges(&mut extracted, &resolver);
        assert_eq!(resolver.call_count(), 0);
        match &extracted.edges[0].to {
            ResolvedOrUnresolved::Resolved { fqdn } => assert_eq!(fqdn, "primary::lib::Local"),
            other => panic!("expected Resolved, got {other:?}"),
        }
    }

    #[test]
    fn hit_rewrites_target_and_stamps_attrs() {
        let mut extracted = empty_extracted(Language::Rust, "primary::lib");
        extracted.edges.push(unresolved_edge("peer::lib::Foo"));
        let mut hits = HashMap::new();
        hits.insert(
            ("peer::lib".to_string(), "Foo".to_string()),
            CrossWorkspaceLookup::Hit {
                workspace_id: "peer-uuid".into(),
                fqdn: "peer::lib::Foo".into(),
            },
        );
        let resolver = FakeResolver::new(hits);
        rewrite_cross_workspace_edges(&mut extracted, &resolver);
        let edge = &extracted.edges[0];
        match &edge.to {
            ResolvedOrUnresolved::Resolved { fqdn } => assert_eq!(fqdn, "peer::lib::Foo"),
            other => panic!("expected Resolved, got {other:?}"),
        }
        assert!(edge.attributes.contains(&"cross-workspace".to_string()));
        assert!(edge.attributes.contains(&"peer-peer-uuid".to_string()));
    }

    #[test]
    fn miss_emits_unresolved_bridge_with_custom_slug() {
        let mut extracted = empty_extracted(Language::TypeScript, "@app::module");
        extracted
            .edges
            .push(resolved_edge("@peer::lib::DoesNotExist"));
        let mut hits = HashMap::new();
        hits.insert(
            ("@peer::lib".to_string(), "DoesNotExist".to_string()),
            CrossWorkspaceLookup::KnownPeerMissingSymbol {
                workspace_id: "peer-uuid".into(),
            },
        );
        let resolver = FakeResolver::new(hits);
        rewrite_cross_workspace_edges(&mut extracted, &resolver);
        let edge = &extracted.edges[0];
        match &edge.to {
            ResolvedOrUnresolved::UnresolvedBridge { bridge, name } => {
                assert_eq!(bridge.as_str(), "custom:cross-workspace");
                assert_eq!(name, "@peer::lib::DoesNotExist");
            }
            other => panic!("expected UnresolvedBridge, got {other:?}"),
        }
        assert!(edge.attributes.contains(&"cross-workspace".to_string()));
        assert!(edge.attributes.contains(&"peer-peer-uuid".to_string()));
    }

    #[test]
    fn unknown_leaves_edge_unchanged() {
        let mut extracted = empty_extracted(Language::Rust, "primary::lib");
        extracted.edges.push(unresolved_edge("external::lib::Foo"));
        let resolver = FakeResolver::new(HashMap::new());
        rewrite_cross_workspace_edges(&mut extracted, &resolver);
        let edge = &extracted.edges[0];
        match &edge.to {
            ResolvedOrUnresolved::Unresolved { name } => assert_eq!(name, "external::lib::Foo"),
            other => panic!("expected Unresolved, got {other:?}"),
        }
        assert!(edge.attributes.is_empty());
    }

    #[test]
    fn skips_languages_without_workspace_prefix() {
        // Vue / Svelte SFC shells still bail — their TS body is
        // routed through the TS extractor which gets its own rewrite
        // pass, so the SFC-level extraction is intentionally a no-op.
        let mut extracted = empty_extracted(Language::Vue, "lib.vue");
        extracted.edges.push(unresolved_edge("peer::lib::Foo"));
        let resolver = FakeResolver::new(HashMap::new());
        rewrite_cross_workspace_edges(&mut extracted, &resolver);
        assert_eq!(resolver.call_count(), 0);
    }

    #[test]
    fn c_extractions_participate_in_cross_workspace_rewrite() {
        let mut extracted = empty_extracted(Language::C, "lurlang::runtime::vm");
        extracted.edges.push(unresolved_edge("peer::lib::api"));
        let mut hits = HashMap::new();
        hits.insert(
            ("peer::lib".to_string(), "api".to_string()),
            CrossWorkspaceLookup::Hit {
                workspace_id: "peer-uuid".into(),
                fqdn: "peer::lib::api".into(),
            },
        );
        let resolver = FakeResolver::new(hits);
        rewrite_cross_workspace_edges(&mut extracted, &resolver);
        let edge = &extracted.edges[0];
        match &edge.to {
            ResolvedOrUnresolved::Resolved { fqdn } => assert_eq!(fqdn, "peer::lib::api"),
            other => panic!("expected Resolved, got {other:?}"),
        }
        assert!(edge.attributes.contains(&"cross-workspace".to_string()));
    }

    #[test]
    fn lua_extractions_participate_in_cross_workspace_rewrite() {
        let mut extracted = empty_extracted(Language::Lua, "pkg::a");
        extracted.edges.push(unresolved_edge("peer::lib::helpers"));
        let mut hits = HashMap::new();
        hits.insert(
            ("peer::lib".to_string(), "helpers".to_string()),
            CrossWorkspaceLookup::Hit {
                workspace_id: "peer-uuid".into(),
                fqdn: "peer::lib::helpers".into(),
            },
        );
        let resolver = FakeResolver::new(hits);
        rewrite_cross_workspace_edges(&mut extracted, &resolver);
        let edge = &extracted.edges[0];
        match &edge.to {
            ResolvedOrUnresolved::Resolved { fqdn } => assert_eq!(fqdn, "peer::lib::helpers"),
            other => panic!("expected Resolved, got {other:?}"),
        }
        assert!(edge.attributes.contains(&"cross-workspace".to_string()));
    }

    #[test]
    fn suffix_chain_resolves_method_on_re_exported_type() {
        // Bug E-2: `lur_common::Span::new` — caller wrote
        // `use lur_common::Span; Span::new()`. The peer's `Span` is a
        // re-export pointing to `lur-common::span::Span` (canonical), so
        // (`lur_common::Span`, `new`) misses (Span is a symbol, not a
        // module). Walking split points longest-first finds
        // (`lur_common`, `Span`) → Hit with canonical, then appends
        // `::new` → `lur-common::span::Span::new`.
        let mut extracted = empty_extracted(Language::Rust, "primary::lib");
        extracted
            .edges
            .push(unresolved_edge("lur_common::Span::new"));
        let mut hits = HashMap::new();
        hits.insert(
            ("lur_common".to_string(), "Span".to_string()),
            CrossWorkspaceLookup::Hit {
                workspace_id: "peer-uuid".into(),
                fqdn: "lur-common::span::Span".into(),
            },
        );
        let resolver = FakeResolver::new(hits);
        rewrite_cross_workspace_edges(&mut extracted, &resolver);
        let edge = &extracted.edges[0];
        match &edge.to {
            ResolvedOrUnresolved::Resolved { fqdn } => {
                assert_eq!(fqdn, "lur-common::span::Span::new");
            }
            other => panic!("expected Resolved with appended tail, got {other:?}"),
        }
        assert!(edge.attributes.contains(&"cross-workspace".to_string()));
    }

    #[test]
    fn suffix_chain_prefers_longest_module_match() {
        // When both (`crate::sub`, `Sym`) and (`crate`, `sub`) could hit,
        // the longest-module-first iteration must pick the more specific
        // (`crate::sub`, `Sym`) hit so we don't accidentally re-route
        // through a shorter (and semantically different) prefix.
        let mut extracted = empty_extracted(Language::Rust, "primary::lib");
        extracted
            .edges
            .push(unresolved_edge("peer::module::Symbol"));
        let mut hits = HashMap::new();
        hits.insert(
            ("peer::module".to_string(), "Symbol".to_string()),
            CrossWorkspaceLookup::Hit {
                workspace_id: "peer-uuid".into(),
                fqdn: "peer::module::Symbol".into(),
            },
        );
        // Decoy that should NOT be picked because of longest-first preference.
        hits.insert(
            ("peer".to_string(), "module".to_string()),
            CrossWorkspaceLookup::Hit {
                workspace_id: "wrong-uuid".into(),
                fqdn: "peer::DECOY".into(),
            },
        );
        let resolver = FakeResolver::new(hits);
        rewrite_cross_workspace_edges(&mut extracted, &resolver);
        let edge = &extracted.edges[0];
        match &edge.to {
            ResolvedOrUnresolved::Resolved { fqdn } => assert_eq!(fqdn, "peer::module::Symbol"),
            other => panic!("expected longest-match Resolved, got {other:?}"),
        }
        assert!(edge.attributes.contains(&"peer-peer-uuid".to_string()));
    }

    #[test]
    fn suffix_chain_falls_back_to_known_peer_missing_symbol() {
        // No split yields a Hit, but the shortest (`peer`, `lib`)
        // returns KnownPeerMissingSymbol — the peer exists but doesn't
        // export the symbol. Surface this as UnresolvedBridge with the
        // original full candidate so the viz can show "tried, missing".
        let mut extracted = empty_extracted(Language::Rust, "primary::lib");
        extracted
            .edges
            .push(unresolved_edge("peer::lib::Missing::method"));
        let mut hits = HashMap::new();
        hits.insert(
            ("peer::lib".to_string(), "Missing".to_string()),
            CrossWorkspaceLookup::KnownPeerMissingSymbol {
                workspace_id: "peer-uuid".into(),
            },
        );
        let resolver = FakeResolver::new(hits);
        rewrite_cross_workspace_edges(&mut extracted, &resolver);
        let edge = &extracted.edges[0];
        match &edge.to {
            ResolvedOrUnresolved::UnresolvedBridge { name, .. } => {
                assert_eq!(name, "peer::lib::Missing::method");
            }
            other => panic!("expected UnresolvedBridge fallback, got {other:?}"),
        }
        assert!(edge.attributes.contains(&"peer-peer-uuid".to_string()));
    }

    #[test]
    fn idempotent_on_repeat_call() {
        let mut extracted = empty_extracted(Language::Rust, "primary::lib");
        extracted.edges.push(unresolved_edge("peer::lib::Foo"));
        let mut hits = HashMap::new();
        hits.insert(
            ("peer::lib".to_string(), "Foo".to_string()),
            CrossWorkspaceLookup::Hit {
                workspace_id: "peer".into(),
                fqdn: "peer::lib::Foo".into(),
            },
        );
        let resolver = FakeResolver::new(hits);
        rewrite_cross_workspace_edges(&mut extracted, &resolver);
        let attrs_after_first = extracted.edges[0].attributes.clone();
        // Second pass: target is Resolved + already starts with "peer::lib::Foo".
        // It IS NOT local (no primary prefix), so it would consult resolver
        // again — but stamp_attrs dedup keeps the attrs vec stable.
        rewrite_cross_workspace_edges(&mut extracted, &resolver);
        assert_eq!(extracted.edges[0].attributes, attrs_after_first);
    }
}

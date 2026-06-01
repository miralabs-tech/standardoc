use super::*;

#[test]
fn clamp_limit_defaults_to_twenty_when_unset() {
    assert_eq!(clamp_limit(None), FIND_SYMBOL_DEFAULT_LIMIT);
}

#[test]
fn clamp_limit_caps_at_max() {
    assert_eq!(clamp_limit(Some(255)), FIND_SYMBOL_MAX_LIMIT);
}

#[test]
fn clamp_limit_floors_at_one_when_zero_requested() {
    assert_eq!(clamp_limit(Some(0)), 1);
}

#[test]
fn indexing_message_includes_progress_when_known() {
    let msg = indexing_in_progress_message(Some((42, 100)));
    assert!(msg.contains("42/100 files"), "got `{msg}`");
}

#[test]
fn indexing_message_omits_progress_when_zero_total() {
    let msg = indexing_in_progress_message(Some((0, 0)));
    assert!(!msg.contains('/'), "got `{msg}`");
}

#[test]
fn relative_fqdn_strips_prefix_with_marker() {
    assert_eq!(
        relative_fqdn("foo::bar::baz::qux", "foo::bar"),
        "::baz::qux",
    );
}

#[test]
fn relative_fqdn_returns_empty_on_self_match() {
    assert_eq!(relative_fqdn("foo::bar", "foo::bar"), "");
}

#[test]
fn relative_fqdn_passes_through_when_prefix_does_not_match() {
    assert_eq!(relative_fqdn("other::lib::x", "foo::bar"), "other::lib::x",);
}

#[test]
fn relative_fqdn_short_circuits_on_empty_anchor() {
    assert_eq!(relative_fqdn("foo::bar::baz", ""), "foo::bar::baz");
}

#[test]
fn relative_fqdn_requires_segment_boundary() {
    // `foo::bar` should NOT match the prefix of `foo::barista` — the
    // boundary is `::`, not raw string prefix.
    assert_eq!(
        relative_fqdn("foo::barista::x", "foo::bar"),
        "foo::barista::x",
    );
}

use standardoc_core::{ScanFilters, cold_start};
use standardoc_lang_provider::WorkspaceProvider;
use std::path::Path;
use tempfile::TempDir;

fn fixture() -> (TempDir, StandardocMcp) {
    let dir = tempfile::tempdir().unwrap();
    let handle = IndexHandle::open(dir.path()).unwrap();
    let provider: Arc<dyn LanguageProvider> = Arc::new(WorkspaceProvider::new());
    let filters = Arc::new(RwLock::new(ScanFilters::load(handle.workspace_root())));
    let mcp = StandardocMcp::new(handle, provider, filters);
    (dir, mcp)
}

fn cold_start_workspace(mcp: &StandardocMcp, root: &Path) {
    let provider = WorkspaceProvider::new();
    let filters = ScanFilters::load(root);
    cold_start::run(&mcp.handle, &provider, &filters).unwrap();
    mcp.index_ready.store(true, Ordering::Release);
}

fn body_text(result: &CallToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|c| match &c.raw {
            rmcp::model::RawContent::Text(t) => Some(t.text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test(flavor = "multi_thread")]
async fn get_context_returns_indexing_in_progress_when_not_ready() {
    let (_dir, mcp) = fixture();
    let result = mcp
        .get_context(Parameters(GetContextParams {
            fqdn: "crate::anything".into(),
            depth: None,
        }))
        .await
        .expect("tool returns Ok with friendly degradation");
    let text = body_text(&result);
    assert!(
        text.contains("Workspace indexing in progress"),
        "expected friendly progress message, got `{text}`"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn find_symbol_returns_indexing_in_progress_when_not_ready() {
    let (_dir, mcp) = fixture();
    let result = mcp
        .find_symbol(Parameters(FindSymbolParams {
            query: "anything".into(),
            limit: None,
            kind: None,
            visibility: None,
            module: None,
            include_external: None,
            exclude_tests: None,
            workspace_id: None,
        }))
        .await
        .expect("tool returns Ok with friendly degradation");
    let text = body_text(&result);
    assert!(
        text.contains("Workspace indexing in progress"),
        "expected friendly progress message, got `{text}`"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn list_symbols_returns_indexing_in_progress_when_not_ready() {
    let (_dir, mcp) = fixture();
    let result = mcp
        .list_symbols(Parameters(ListSymbolsParams {
            kind: None,
            visibility: None,
            module: None,
            limit: None,
            include_external: None,
            exclude_tests: None,
            cursor: None,
            workspace_id: None,
        }))
        .await
        .expect("tool returns Ok with friendly degradation");
    assert!(body_text(&result).contains("Workspace indexing in progress"));
}

/// Envelope-shape check: an empty workspace must still return
/// `{"items": [...], "next_cursor": ...}`, not a bare array. This
/// is the contract the playground + ext rely on to walk pages.
#[tokio::test(flavor = "multi_thread")]
async fn list_symbols_returns_page_envelope_when_empty() {
    let (dir, mcp) = fixture();
    cold_start_workspace(&mcp, dir.path());
    let result = mcp
        .list_symbols(Parameters(ListSymbolsParams {
            kind: None,
            visibility: None,
            module: None,
            limit: None,
            include_external: Some(false),
            exclude_tests: None,
            cursor: None,
            workspace_id: None,
        }))
        .await
        .unwrap();
    let text = body_text(&result);
    let json: serde_json::Value = serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("envelope must be valid JSON, got `{text}`: {e}"));
    assert!(
        json.get("items").is_some_and(serde_json::Value::is_array),
        "envelope must carry an `items` array, got `{text}`"
    );
    assert!(
        json.get("next_cursor").is_some(),
        "envelope must carry a `next_cursor` field (null when no more pages), got `{text}`"
    );
    // Empty workspace → empty items + null cursor.
    assert_eq!(json["items"].as_array().unwrap().len(), 0);
    assert!(json["next_cursor"].is_null());
}

/// The `cursor` param must be plumbed through the JsonSchema and
/// not rejected as an unknown parameter. We don't seed real
/// symbols here — just verify the daemon accepts the cursor and
/// returns a well-formed envelope (the core layer is exhaustively
/// tested in `standardoc-core::query::tests::list_symbols_cursor_*`).
#[tokio::test(flavor = "multi_thread")]
async fn list_symbols_accepts_cursor_param() {
    let (dir, mcp) = fixture();
    cold_start_workspace(&mcp, dir.path());
    let result = mcp
        .list_symbols(Parameters(ListSymbolsParams {
            kind: None,
            visibility: None,
            module: None,
            limit: Some(2),
            include_external: Some(false),
            exclude_tests: None,
            cursor: Some("crate::anchor".into()),
            workspace_id: None,
        }))
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_str(&body_text(&result)).unwrap();
    assert!(json["items"].is_array());
    assert!(json["next_cursor"].is_null());
}

#[tokio::test(flavor = "multi_thread")]
async fn find_symbols_by_pattern_returns_indexing_in_progress_when_not_ready() {
    let (_dir, mcp) = fixture();
    let result = mcp
        .find_symbols_by_pattern(Parameters(FindSymbolsByPatternParams {
            pattern: "anything_*".into(),
            kind: None,
            visibility: None,
            module: None,
            limit: None,
            include_external: None,
            exclude_tests: None,
            workspace_id: None,
        }))
        .await
        .expect("tool returns Ok with friendly degradation");
    assert!(body_text(&result).contains("Workspace indexing in progress"));
}

#[test]
fn parse_kind_recognises_every_ir_variant() {
    assert!(parse_kind("callable").is_ok());
    assert!(parse_kind("type").is_ok());
    assert!(parse_kind("value").is_ok());
    assert!(parse_kind("module").is_ok());
    assert!(parse_kind("macro").is_ok());
}

#[test]
fn parse_kind_rejects_unknown() {
    assert!(parse_kind("class").is_err());
    assert!(parse_kind("").is_err());
}

#[test]
fn parse_visibility_recognises_every_ir_variant() {
    assert!(parse_visibility("public").is_ok());
    assert!(parse_visibility("private").is_ok());
    assert!(parse_visibility("crate").is_ok());
    assert!(parse_visibility("protected").is_ok());
}

#[test]
fn parse_visibility_rejects_unknown() {
    assert!(parse_visibility("internal").is_err());
    assert!(parse_visibility("").is_err());
}

#[test]
fn parse_filter_propagates_module_string_unchanged() {
    let f = parse_filter(
        Some("callable"),
        Some("private"),
        Some("crate::a".into()),
        None,
        None,
    )
    .unwrap();
    assert_eq!(f.kind, Some(Kind::Callable));
    assert_eq!(f.visibility, Some(Visibility::Private));
    assert_eq!(f.module.as_deref(), Some("crate::a"));
    assert!(
        f.include_external,
        "omitting include_external must default to true (S3-G include externals by default)"
    );
}

#[test]
fn parse_filter_all_none_yields_empty_filter() {
    let f = parse_filter(None, None, None, None, None).unwrap();
    assert_eq!(f, SymbolFilter::default());
}

#[test]
fn parse_filter_propagates_workspace_id_when_supplied() {
    // L3e-2: workspace_id flows through parse_filter unchanged so
    // downstream SQL narrows to that peer's rows.
    let f = parse_filter(None, None, None, None, Some("peer-uuid-xyz".into())).unwrap();
    assert_eq!(f.workspace_id.as_deref(), Some("peer-uuid-xyz"));
    assert_eq!(f.effective_workspace_id(), "peer-uuid-xyz");
}

#[test]
fn parse_filter_propagates_include_external_false() {
    let f = parse_filter(None, None, None, Some(false), None).unwrap();
    assert!(
        !f.include_external,
        "explicit false must scope queries to workspace-only symbols"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn get_context_returns_null_when_fqdn_unknown() {
    let (dir, mcp) = fixture();
    cold_start_workspace(&mcp, dir.path());
    let result = mcp
        .get_context(Parameters(GetContextParams {
            fqdn: "crate::ghost".into(),
            depth: Some(1),
        }))
        .await
        .unwrap();
    let text = body_text(&result);
    assert_eq!(
        text.trim(),
        "null",
        "unknown FQDN must return JSON null, got `{text}`"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn find_symbol_returns_empty_results_for_blank_query() {
    let (dir, mcp) = fixture();
    cold_start_workspace(&mcp, dir.path());
    let result = mcp
        .find_symbol(Parameters(FindSymbolParams {
            query: "   ".into(),
            limit: None,
            kind: None,
            visibility: None,
            module: None,
            include_external: None,
            exclude_tests: None,
            workspace_id: None,
        }))
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_str(&body_text(&result)).unwrap();
    assert!(
        json["results"].as_array().is_some_and(|a| a.is_empty()),
        "blank query must short-circuit to empty `results`, got `{json}`"
    );
    assert!(json["did_you_mean"].as_array().is_some_and(|a| a.is_empty()));
}

#[tokio::test(flavor = "multi_thread")]
async fn find_symbols_by_pattern_returns_empty_results_for_blank_pattern() {
    let (dir, mcp) = fixture();
    cold_start_workspace(&mcp, dir.path());
    let result = mcp
        .find_symbols_by_pattern(Parameters(FindSymbolsByPatternParams {
            pattern: "   ".into(),
            kind: None,
            visibility: None,
            module: None,
            limit: None,
            include_external: None,
            exclude_tests: None,
            workspace_id: None,
        }))
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_str(&body_text(&result)).unwrap();
    assert!(json["results"].as_array().is_some_and(|a| a.is_empty()));
    assert!(json["did_you_mean"].as_array().is_some_and(|a| a.is_empty()));
}

#[tokio::test(flavor = "multi_thread")]
async fn find_similar_symbols_returns_indexing_in_progress_when_not_ready() {
    let (_dir, mcp) = fixture();
    let result = mcp
        .find_similar_symbols(Parameters(FindSimilarSymbolsParams {
            reference: "anything".into(),
            threshold: None,
            limit: None,
            kind: None,
            visibility: None,
            module: None,
            include_external: None,
        }))
        .await
        .expect("tool returns Ok with friendly degradation");
    assert!(body_text(&result).contains("Workspace indexing in progress"));
}

#[tokio::test(flavor = "multi_thread")]
async fn find_similar_symbols_blank_reference_returns_empty_results() {
    let (dir, mcp) = fixture();
    cold_start_workspace(&mcp, dir.path());
    let result = mcp
        .find_similar_symbols(Parameters(FindSimilarSymbolsParams {
            reference: "   ".into(),
            threshold: None,
            limit: None,
            kind: None,
            visibility: None,
            module: None,
            include_external: None,
        }))
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_str(&body_text(&result)).unwrap();
    assert!(json["results"].as_array().is_some_and(|a| a.is_empty()));
}

#[tokio::test(flavor = "multi_thread")]
async fn find_similar_symbols_threshold_above_one_rejected() {
    let (dir, mcp) = fixture();
    cold_start_workspace(&mcp, dir.path());
    let result = mcp
        .find_similar_symbols(Parameters(FindSimilarSymbolsParams {
            reference: "foo".into(),
            threshold: Some(1.5),
            limit: None,
            kind: None,
            visibility: None,
            module: None,
            include_external: None,
        }))
        .await;
    assert!(
        result.is_err(),
        "out-of-range threshold must be rejected with ErrorData"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn find_similar_symbols_threshold_negative_rejected() {
    let (dir, mcp) = fixture();
    cold_start_workspace(&mcp, dir.path());
    let result = mcp
        .find_similar_symbols(Parameters(FindSimilarSymbolsParams {
            reference: "foo".into(),
            threshold: Some(-0.1),
            limit: None,
            kind: None,
            visibility: None,
            module: None,
            include_external: None,
        }))
        .await;
    assert!(result.is_err(), "negative threshold must be rejected");
}

#[tokio::test(flavor = "multi_thread")]
async fn find_similar_symbols_invalid_kind_filter_returns_error() {
    let (dir, mcp) = fixture();
    cold_start_workspace(&mcp, dir.path());
    let result = mcp
        .find_similar_symbols(Parameters(FindSimilarSymbolsParams {
            reference: "anything".into(),
            threshold: None,
            limit: None,
            kind: Some("class".into()),
            visibility: None,
            module: None,
            include_external: None,
        }))
        .await;
    assert!(result.is_err(), "invalid `kind` filter must be rejected");
}

#[test]
fn parse_threshold_defaults_to_zero_eight_when_none() {
    let got = parse_threshold(None).unwrap();
    assert!((got - FIND_SIMILAR_DEFAULT_THRESHOLD).abs() < f32::EPSILON);
}

#[test]
fn parse_threshold_accepts_zero_and_one_inclusive() {
    assert!(parse_threshold(Some(0.0)).is_ok());
    assert!(parse_threshold(Some(1.0)).is_ok());
}

#[test]
fn parse_threshold_rejects_nan_and_infinity() {
    assert!(parse_threshold(Some(f32::NAN)).is_err());
    assert!(parse_threshold(Some(f32::INFINITY)).is_err());
    assert!(parse_threshold(Some(f32::NEG_INFINITY)).is_err());
}

#[test]
fn glob_core_text_strips_star_wildcard() {
    assert_eq!(glob_core_text("*to_token_string*"), "to_token_string");
}

#[test]
fn glob_core_text_strips_question_and_bracket_wildcards_but_keeps_inner_chars() {
    // Brackets themselves are stripped ; the character class content
    // is kept verbatim — strsim still benefits even from `[abc]`
    // alternatives rather than dropping the whole group.
    assert_eq!(glob_core_text("get_?[abc]_value"), "get_abc_value");
}

#[test]
fn glob_core_text_empty_for_only_wildcards() {
    assert_eq!(glob_core_text("***"), "");
    assert_eq!(glob_core_text("?*[]"), "");
}

#[test]
fn glob_core_text_preserves_alphanumeric_and_separators() {
    assert_eq!(
        glob_core_text("standardoc-cli::main"),
        "standardoc-cli::main"
    );
}

#[test]
fn normalize_fqdn_replaces_dot_with_double_colon() {
    assert_eq!(
        normalize_fqdn("StandardocMcp.find_symbol"),
        "StandardocMcp::find_symbol"
    );
    assert_eq!(
        normalize_fqdn("crate.mod.Type.method"),
        "crate::mod::Type::method"
    );
}

#[test]
fn normalize_fqdn_is_idempotent_on_double_colon_form() {
    let canonical = "standardoc_core::query::search_text";
    assert_eq!(normalize_fqdn(canonical), canonical);
}

#[test]
fn normalize_fqdn_preserves_other_separators_and_hyphens() {
    // Hyphens (crate names like standardoc-cli) and slashes (TS
    // package paths like @scope/pkg) must survive.
    assert_eq!(
        normalize_fqdn("standardoc-cli::main"),
        "standardoc-cli::main"
    );
    assert_eq!(
        normalize_fqdn("@app/web::module::foo"),
        "@app/web::module::foo"
    );
}

#[test]
fn normalize_fqdn_handles_empty_input() {
    assert_eq!(normalize_fqdn(""), "");
}

#[test]
fn normalize_fqdn_collapses_consecutive_dots_into_quad_colons() {
    // Documents the literal-replace behaviour : malformed input
    // produces literal `::::`. We don't attempt to fix user
    // mistakes ; the downstream exact-match query will fail with
    // a clean "no symbol found" instead of silently passing.
    assert_eq!(normalize_fqdn("foo..bar"), "foo::::bar");
}

#[tokio::test(flavor = "multi_thread")]
async fn find_symbol_invalid_kind_filter_returns_error() {
    let (dir, mcp) = fixture();
    cold_start_workspace(&mcp, dir.path());
    let result = mcp
        .find_symbol(Parameters(FindSymbolParams {
            query: "anything".into(),
            limit: None,
            kind: Some("class".into()),
            visibility: None,
            module: None,
            include_external: None,
            exclude_tests: None,
            workspace_id: None,
        }))
        .await;
    // Invalid filter is a parameter error — surfaces as Err on the
    // tool invocation, NOT a graceful CallToolResult.
    assert!(
        result.is_err(),
        "invalid `kind` must be rejected with ErrorData"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn get_body_returns_indexing_in_progress_when_not_ready() {
    let (_dir, mcp) = fixture();
    let result = mcp
        .get_body(Parameters(GetBodyParams {
            fqdn: "crate::foo".into(),
            max_lines: None,
            strip_attrs: None,
            signature_only: None,
            strip_inline_comments: None,
        }))
        .await
        .expect("tool returns Ok with friendly degradation");
    assert!(body_text(&result).contains("Workspace indexing in progress"));
}

#[tokio::test(flavor = "multi_thread")]
async fn compute_routing_hint_is_none_for_depth_one() {
    let (_dir, mcp) = fixture();
    assert_eq!(mcp.compute_routing_hint("crate::any", 1, 1_000), None);
}

#[tokio::test(flavor = "multi_thread")]
async fn compute_routing_hint_fires_for_naked_depth_two() {
    let (_dir, mcp) = fixture();
    let hint = mcp.compute_routing_hint("crate::any", 2, 1_000);
    assert!(hint.is_some(), "naked depth=2 must surface a routing hint");
    let msg = hint.unwrap();
    assert!(msg.contains("depth=2"), "got `{msg}`");
    assert!(msg.contains("depth=1"), "got `{msg}`");
}

#[tokio::test(flavor = "multi_thread")]
async fn compute_routing_hint_silent_after_recent_depth_one() {
    let (_dir, mcp) = fixture();
    let now = 10_000_i64;
    mcp.record_recent_depth1("crate::scoped", now - 60);
    // 60 s after a depth=1 call, depth=2 should be hint-free.
    assert_eq!(
        mcp.compute_routing_hint("crate::scoped", 2, now),
        None,
        "depth=2 within the 5 min window must NOT trigger the hint"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn compute_routing_hint_fires_again_when_window_expires() {
    let (_dir, mcp) = fixture();
    let now = 10_000_i64;
    mcp.record_recent_depth1("crate::stale", now - 600);
    // 10 min later, the prior depth=1 is outside the 5 min window.
    let hint = mcp.compute_routing_hint("crate::stale", 2, now);
    assert!(
        hint.is_some(),
        "stale scoping pass must not silence the hint"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn record_recent_depth_one_evicts_entries_older_than_retention() {
    let (_dir, mcp) = fixture();
    let now = 100_000_i64;
    mcp.record_recent_depth1("crate::ancient", now - 5_000);
    // Insert another entry far enough in the future that the retention
    // window (1800 s) drops the ancient one on the next sweep.
    mcp.record_recent_depth1("crate::fresh", now + 2_000);
    let (has_ancient, has_fresh) = {
        let guard = mcp.recent_depth1.lock().unwrap();
        (
            guard.contains_key("crate::ancient"),
            guard.contains_key("crate::fresh"),
        )
    };
    assert!(!has_ancient);
    assert!(has_fresh);
}

#[tokio::test(flavor = "multi_thread")]
async fn get_body_returns_null_when_fqdn_unknown() {
    let (dir, mcp) = fixture();
    cold_start_workspace(&mcp, dir.path());
    let result = mcp
        .get_body(Parameters(GetBodyParams {
            fqdn: "crate::nope::never_indexed".into(),
            max_lines: None,
            strip_attrs: None,
            signature_only: None,
            strip_inline_comments: None,
        }))
        .await
        .unwrap();
    assert_eq!(body_text(&result).trim(), "null");
}

#[tokio::test(flavor = "multi_thread")]
async fn current_revision_returns_zero_on_fresh_index() {
    let (_dir, mcp) = fixture();
    let result = mcp.current_revision().await.unwrap();
    let body = body_text(&result);
    assert!(body.contains("\"revision\": 0"), "got `{body}`");
}

#[tokio::test(flavor = "multi_thread")]
async fn current_revision_advances_after_cold_start_writes() {
    let (dir, mcp) = fixture();
    cold_start_workspace(&mcp, dir.path());
    let result = mcp.current_revision().await.unwrap();
    let body = body_text(&result);
    // Empty workspace = 0 writes = revision stays 0. We rely on the field
    // shape rather than the exact value here.
    assert!(body.contains("\"revision\""), "got `{body}`");
}

#[tokio::test(flavor = "multi_thread")]
async fn current_revision_omits_rag_field_post_removal() {
    let (_dir, mcp) = fixture();
    let result = mcp.current_revision().await.unwrap();
    let body = body_text(&result);
    // RAG layer was removed — the `rag` field must no longer be
    // surfaced. Consumers that previously read `rag.enabled` get a
    // breaking absence rather than a stale `false`.
    assert!(
        !body.contains("\"rag\""),
        "rag capability block must be gone, got `{body}`"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn current_revision_reports_indexing_not_ready_before_cold_start() {
    let (_dir, mcp) = fixture();
    let result = mcp.current_revision().await.unwrap();
    let body = body_text(&result);
    assert!(
        body.contains("\"indexing\""),
        "expected indexing block, got `{body}`"
    );
    assert!(body.contains("\"ready\": false"), "got `{body}`");
}

#[tokio::test(flavor = "multi_thread")]
async fn current_revision_reports_indexing_ready_after_cold_start() {
    let (dir, mcp) = fixture();
    cold_start_workspace(&mcp, dir.path());
    let result = mcp.current_revision().await.unwrap();
    let body = body_text(&result);
    assert!(body.contains("\"ready\": true"), "got `{body}`");
}

#[tokio::test(flavor = "multi_thread")]
async fn current_revision_reports_watcher_active_when_workspace_locked() {
    let (_dir, mcp) = fixture();
    // `watcher.active` now reports whether the WORKSPACE is being watched,
    // not whether THIS process wired a watcher slot. The fixture opens
    // the handle via `IndexHandle::open` (primary) so it owns the fs4
    // lock — a primary writer is present — and `active` reads `true` even
    // though no watcher handle was wired into the slot.
    let result = mcp.current_revision().await.unwrap();
    let body = body_text(&result);
    assert!(body.contains("\"watcher\""), "got `{body}`");
    assert!(body.contains("\"active\": true"), "got `{body}`");
}

#[tokio::test(flavor = "multi_thread")]
async fn stage3e3_current_revision_workspace_kind_null_pre_cold_start() {
    let (_dir, mcp) = fixture();
    let result = mcp.current_revision().await.unwrap();
    let body = body_text(&result);
    assert!(body.contains("\"workspace\""), "got `{body}`");
    // No discovery has run yet → null.
    assert!(body.contains("\"kind\": null"), "got `{body}`");
}

#[tokio::test(flavor = "multi_thread")]
async fn stage3e3_current_revision_workspace_kind_is_null_when_no_manifest() {
    let (dir, mcp) = fixture();
    cold_start_workspace(&mcp, dir.path());
    let result = mcp.current_revision().await.unwrap();
    let body = body_text(&result);
    // Cold-start has run, but the fixture has no workspace manifest
    // at root → discovery deletes the row → `kind: null`. (Post-
    // revert of `WorkspaceKind::Single` — aligns with
    // standarbuild-detect 0.3.)
    assert!(body.contains("\"workspace\""), "got `{body}`");
    assert!(body.contains("\"kind\": null"), "got `{body}`");
}

#[tokio::test(flavor = "multi_thread")]
async fn check_stale_empty_fetched_returns_empty_array() {
    let (dir, mcp) = fixture();
    cold_start_workspace(&mcp, dir.path());
    let result = mcp
        .check_stale(Parameters(CheckStaleParams { fetched: vec![] }))
        .await
        .unwrap();
    assert_eq!(body_text(&result).trim(), "[]");
}

#[tokio::test(flavor = "multi_thread")]
async fn check_stale_unknown_fqdn_marked_missing() {
    let (dir, mcp) = fixture();
    cold_start_workspace(&mcp, dir.path());
    let result = mcp
        .check_stale(Parameters(CheckStaleParams {
            fetched: vec![FetchedEntry {
                fqdn: "crate::nope::never_indexed".into(),
                fetched_at_revision: 5,
            }],
        }))
        .await
        .unwrap();
    let body = body_text(&result);
    assert!(body.contains("\"status\": \"missing\""), "got `{body}`");
    assert!(
        body.contains("\"last_modified_revision\": null"),
        "got `{body}`"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn check_stale_returns_indexing_in_progress_when_not_ready() {
    let (_dir, mcp) = fixture();
    let result = mcp
        .check_stale(Parameters(CheckStaleParams {
            fetched: vec![FetchedEntry {
                fqdn: "crate::foo".into(),
                fetched_at_revision: 0,
            }],
        }))
        .await
        .expect("tool returns Ok with friendly degradation");
    assert!(body_text(&result).contains("Workspace indexing in progress"));
}

#[tokio::test(flavor = "multi_thread")]
async fn find_symbol_workspace_id_param_narrows_to_named_peer() {
    // L3e-2: passing `workspace_id` through the MCP tool reaches
    // the SQL filter. We don't need to seed peer rows here — the
    // core tests already cover that path. A non-existent peer
    // workspace_id must yield an empty result (proves the filter
    // is wired and primary rows aren't leaking through).
    use std::fs;
    let (dir, mcp) = fixture();
    fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(
        dir.path().join("src").join("lib.rs"),
        "pub fn hello_marker() {}",
    )
    .unwrap();
    cold_start_workspace(&mcp, dir.path());

    let default_scope = mcp
        .find_symbol(Parameters(FindSymbolParams {
            query: "hello_marker".into(),
            limit: None,
            kind: None,
            visibility: None,
            module: None,
            include_external: None,
            exclude_tests: None,
            workspace_id: None,
        }))
        .await
        .unwrap();
    let body = body_text(&default_scope);
    assert!(
        body.contains("hello_marker"),
        "default scope must surface the primary symbol, got `{body}`"
    );

    let peer_scope = mcp
        .find_symbol(Parameters(FindSymbolParams {
            query: "hello_marker".into(),
            limit: None,
            kind: None,
            visibility: None,
            module: None,
            include_external: None,
            exclude_tests: None,
            workspace_id: Some("nonexistent-peer-uuid".into()),
        }))
        .await
        .unwrap();
    let body = body_text(&peer_scope);
    // The empty-result envelope is `did_you_mean` (DYM kicks in when
    // results vector is empty), not a leaked primary row.
    assert!(
        !body.contains("hello_marker") || body.contains("did_you_mean"),
        "peer scope must NOT leak the primary hello_marker symbol \
             (DYM is fine — it operates on names, not workspace), got `{body}`"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn link_workspace_returns_workspace_id_for_existing_path() {
    let (dir, mcp) = fixture();
    let peer = tempfile::tempdir().unwrap();
    let result = mcp
        .link_workspace(Parameters(LinkWorkspaceParams {
            path: peer.path().to_string_lossy().into_owned(),
            direction: "in".into(),
            indexing_mode: None,
        }))
        .await
        .expect("link_workspace ok");
    let body = body_text(&result);
    assert!(body.contains("\"workspace_id\""), "got `{body}`");
    assert!(body.contains("\"direction\": \"in\""), "got `{body}`");
    drop(dir);
}

#[tokio::test(flavor = "multi_thread")]
async fn link_workspace_rejects_missing_path_with_did_you_mean() {
    let (_dir, mcp) = fixture();
    let parent = tempfile::tempdir().unwrap();
    std::fs::create_dir(parent.path().join("projects")).unwrap();
    let typo = parent.path().join("projcts");
    let err = mcp
        .link_workspace(Parameters(LinkWorkspaceParams {
            path: typo.to_string_lossy().into_owned(),
            direction: "in".into(),
            indexing_mode: None,
        }))
        .await
        .expect_err("missing path must surface invalid_params");
    let data = format!("{:?}", err.data);
    assert!(data.contains("did_you_mean"), "got `{data}`");
    assert!(data.contains("projects"), "got `{data}`");
}

#[tokio::test(flavor = "multi_thread")]
async fn link_workspace_rejects_unknown_direction() {
    let (_dir, mcp) = fixture();
    let peer = tempfile::tempdir().unwrap();
    let err = mcp
        .link_workspace(Parameters(LinkWorkspaceParams {
            path: peer.path().to_string_lossy().into_owned(),
            direction: "sideways".into(),
            indexing_mode: None,
        }))
        .await
        .expect_err("bogus direction must be rejected");
    assert!(format!("{err}").contains("direction"));
}

#[tokio::test(flavor = "multi_thread")]
async fn list_linked_workspaces_returns_empty_array_on_fresh_index() {
    let (_dir, mcp) = fixture();
    let result = mcp
        .list_linked_workspaces()
        .await
        .expect("list_linked_workspaces ok");
    let body = body_text(&result);
    assert!(body.contains("\"workspaces\""), "got `{body}`");
    // Fresh index = no rows.
    assert!(body.contains("\"workspaces\": []"), "got `{body}`");
}

#[tokio::test(flavor = "multi_thread")]
async fn unlink_workspace_after_link_removes_row() {
    let (_dir, mcp) = fixture();
    let peer = tempfile::tempdir().unwrap();
    let link = mcp
        .link_workspace(Parameters(LinkWorkspaceParams {
            path: peer.path().to_string_lossy().into_owned(),
            direction: "out".into(),
            indexing_mode: None,
        }))
        .await
        .expect("link ok");
    let link_body = body_text(&link);
    let workspace_id = link_body
        .split("\"workspace_id\": \"")
        .nth(1)
        .and_then(|tail| tail.split('"').next())
        .expect("workspace_id present")
        .to_string();

    let _ = mcp
        .unlink_workspace(Parameters(UnlinkWorkspaceParams {
            workspace_id: workspace_id.clone(),
        }))
        .await
        .expect("unlink ok");

    let list = mcp.list_linked_workspaces().await.expect("list ok");
    let list_body = body_text(&list);
    assert!(
        !list_body.contains(&workspace_id),
        "workspace must be gone after unlink, got `{list_body}`"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn link_workspace_in_direction_registers_peer_with_live_watcher() {
    // L3d-3: when the live watcher is booted, linking a peer with
    // direction=in pushes a PeerRoot into the watcher's registry so
    // the dispatch loop starts routing the peer's events
    // immediately (no cold_start needed).
    use standardoc_core::spawn_watcher;

    let (_dir, mcp) = fixture();
    let peer = tempfile::tempdir().unwrap();

    // Seed the watcher slot — the default fixture leaves it None.
    let watcher = spawn_watcher(
        mcp.handle.clone(),
        Arc::clone(&mcp.provider),
        Arc::clone(&mcp.filters),
    )
    .expect("watcher boot");
    {
        let slot = mcp.watcher_slot();
        let mut guard = slot.lock().unwrap();
        *guard = Some(watcher);
    }

    let link = mcp
        .link_workspace(Parameters(LinkWorkspaceParams {
            path: peer.path().to_string_lossy().into_owned(),
            direction: "in".into(),
            indexing_mode: None,
        }))
        .await
        .expect("link ok");
    let workspace_id = body_text(&link)
        .split("\"workspace_id\": \"")
        .nth(1)
        .and_then(|tail| tail.split('"').next())
        .expect("workspace_id present")
        .to_string();

    let slot = mcp.watcher_slot();
    let guard = slot.lock().unwrap();
    let snapshot = guard.as_ref().expect("watcher present").peers_snapshot();
    assert_eq!(snapshot.len(), 1, "peer must be registered");
    assert_eq!(snapshot[0].workspace_id, workspace_id);
}

#[tokio::test(flavor = "multi_thread")]
async fn link_workspace_out_direction_skips_watcher_registration() {
    // L3d-3: direction=out means the peer reads us, not us reading
    // them — there is nothing to watch on their side, so the
    // watcher registry stays empty even when the slot is booted.
    use standardoc_core::spawn_watcher;

    let (_dir, mcp) = fixture();
    let peer = tempfile::tempdir().unwrap();

    let watcher = spawn_watcher(
        mcp.handle.clone(),
        Arc::clone(&mcp.provider),
        Arc::clone(&mcp.filters),
    )
    .expect("watcher boot");
    {
        let slot = mcp.watcher_slot();
        let mut guard = slot.lock().unwrap();
        *guard = Some(watcher);
    }

    let _ = mcp
        .link_workspace(Parameters(LinkWorkspaceParams {
            path: peer.path().to_string_lossy().into_owned(),
            direction: "out".into(),
            indexing_mode: None,
        }))
        .await
        .expect("link ok");

    let slot = mcp.watcher_slot();
    let guard = slot.lock().unwrap();
    let snapshot = guard.as_ref().expect("watcher present").peers_snapshot();
    assert!(
        snapshot.is_empty(),
        "Out direction must not register a peer; got {snapshot:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn unlink_workspace_removes_peer_from_live_watcher() {
    // L3d-3: the unlink handler drops the peer from the live
    // watcher registry in addition to the catalog write.
    use standardoc_core::spawn_watcher;

    let (_dir, mcp) = fixture();
    let peer = tempfile::tempdir().unwrap();

    let watcher = spawn_watcher(
        mcp.handle.clone(),
        Arc::clone(&mcp.provider),
        Arc::clone(&mcp.filters),
    )
    .expect("watcher boot");
    {
        let slot = mcp.watcher_slot();
        let mut guard = slot.lock().unwrap();
        *guard = Some(watcher);
    }

    let link = mcp
        .link_workspace(Parameters(LinkWorkspaceParams {
            path: peer.path().to_string_lossy().into_owned(),
            direction: "in".into(),
            indexing_mode: None,
        }))
        .await
        .expect("link ok");
    let workspace_id = body_text(&link)
        .split("\"workspace_id\": \"")
        .nth(1)
        .and_then(|tail| tail.split('"').next())
        .expect("workspace_id present")
        .to_string();

    let _ = mcp
        .unlink_workspace(Parameters(UnlinkWorkspaceParams {
            workspace_id: workspace_id.clone(),
        }))
        .await
        .expect("unlink ok");

    let slot = mcp.watcher_slot();
    let guard = slot.lock().unwrap();
    let snapshot = guard.as_ref().expect("watcher present").peers_snapshot();
    assert!(
        snapshot.is_empty(),
        "peer must be gone from watcher after unlink"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn set_link_direction_out_to_in_adds_peer_to_live_watcher() {
    // post-3b-7-b finalize: a peer linked with direction=Out is NOT
    // watched (Out means the peer reads us). Flipping to direction=in
    // must register the peer on the live watcher so subsequent file
    // changes flow through dispatch.
    use standardoc_core::spawn_watcher;

    let (_dir, mcp) = fixture();
    let peer = tempfile::tempdir().unwrap();

    let watcher = spawn_watcher(
        mcp.handle.clone(),
        Arc::clone(&mcp.provider),
        Arc::clone(&mcp.filters),
    )
    .expect("watcher boot");
    {
        let slot = mcp.watcher_slot();
        let mut guard = slot.lock().unwrap();
        *guard = Some(watcher);
    }

    let link = mcp
        .link_workspace(Parameters(LinkWorkspaceParams {
            path: peer.path().to_string_lossy().into_owned(),
            direction: "out".into(),
            indexing_mode: None,
        }))
        .await
        .expect("link ok");
    let workspace_id = body_text(&link)
        .split("\"workspace_id\": \"")
        .nth(1)
        .and_then(|tail| tail.split('"').next())
        .expect("workspace_id present")
        .to_string();

    // Pre-condition: Out direction means NO peer is registered.
    {
        let slot = mcp.watcher_slot();
        let guard = slot.lock().unwrap();
        assert!(
            guard.as_ref().unwrap().peers_snapshot().is_empty(),
            "Out direction must not register a peer"
        );
    }

    let response = mcp
        .set_link_direction(Parameters(SetLinkDirectionParams {
            workspace_id: workspace_id.clone(),
            direction: "in".into(),
        }))
        .await
        .expect("set_link_direction ok");
    let body = body_text(&response);
    assert!(
        body.contains("\"previous_direction\": \"out\""),
        "got `{body}`"
    );
    assert!(body.contains("\"new_direction\": \"in\""), "got `{body}`");

    let slot = mcp.watcher_slot();
    let guard = slot.lock().unwrap();
    let snapshot = guard.as_ref().unwrap().peers_snapshot();
    assert_eq!(snapshot.len(), 1, "Out → In must register the peer");
    assert_eq!(snapshot[0].workspace_id, workspace_id);
}

#[tokio::test(flavor = "multi_thread")]
async fn set_link_direction_in_to_out_removes_peer_from_live_watcher() {
    // Inverse of the above: In → Out must unregister the peer.
    use standardoc_core::spawn_watcher;

    let (_dir, mcp) = fixture();
    let peer = tempfile::tempdir().unwrap();

    let watcher = spawn_watcher(
        mcp.handle.clone(),
        Arc::clone(&mcp.provider),
        Arc::clone(&mcp.filters),
    )
    .expect("watcher boot");
    {
        let slot = mcp.watcher_slot();
        let mut guard = slot.lock().unwrap();
        *guard = Some(watcher);
    }

    let link = mcp
        .link_workspace(Parameters(LinkWorkspaceParams {
            path: peer.path().to_string_lossy().into_owned(),
            direction: "in".into(),
            indexing_mode: None,
        }))
        .await
        .expect("link ok");
    let workspace_id = body_text(&link)
        .split("\"workspace_id\": \"")
        .nth(1)
        .and_then(|tail| tail.split('"').next())
        .expect("workspace_id present")
        .to_string();

    // Pre-condition: In direction registered the peer (L3d-3).
    {
        let slot = mcp.watcher_slot();
        let guard = slot.lock().unwrap();
        assert_eq!(guard.as_ref().unwrap().peers_snapshot().len(), 1);
    }

    let _ = mcp
        .set_link_direction(Parameters(SetLinkDirectionParams {
            workspace_id: workspace_id.clone(),
            direction: "out".into(),
        }))
        .await
        .expect("set_link_direction ok");

    let slot = mcp.watcher_slot();
    let guard = slot.lock().unwrap();
    assert!(
        guard.as_ref().unwrap().peers_snapshot().is_empty(),
        "In → Out must unregister the peer"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn set_link_direction_same_side_transition_is_watcher_noop() {
    // In → Bidirectional: both directions watch the peer, so the
    // watcher registry must stay at 1 entry (NOT 0, NOT 2).
    use standardoc_core::spawn_watcher;

    let (_dir, mcp) = fixture();
    let peer = tempfile::tempdir().unwrap();

    let watcher = spawn_watcher(
        mcp.handle.clone(),
        Arc::clone(&mcp.provider),
        Arc::clone(&mcp.filters),
    )
    .expect("watcher boot");
    {
        let slot = mcp.watcher_slot();
        let mut guard = slot.lock().unwrap();
        *guard = Some(watcher);
    }

    let link = mcp
        .link_workspace(Parameters(LinkWorkspaceParams {
            path: peer.path().to_string_lossy().into_owned(),
            direction: "in".into(),
            indexing_mode: None,
        }))
        .await
        .expect("link ok");
    let workspace_id = body_text(&link)
        .split("\"workspace_id\": \"")
        .nth(1)
        .and_then(|tail| tail.split('"').next())
        .expect("workspace_id present")
        .to_string();

    let _ = mcp
        .set_link_direction(Parameters(SetLinkDirectionParams {
            workspace_id: workspace_id.clone(),
            direction: "bidirectional".into(),
        }))
        .await
        .expect("set_link_direction ok");

    let slot = mcp.watcher_slot();
    let guard = slot.lock().unwrap();
    let snapshot = guard.as_ref().unwrap().peers_snapshot();
    assert_eq!(
        snapshot.len(),
        1,
        "same-side transition must not change registry size"
    );
    assert_eq!(snapshot[0].workspace_id, workspace_id);
}

#[tokio::test(flavor = "multi_thread")]
async fn set_link_direction_returns_invalid_params_for_unknown_workspace_id() {
    let (_dir, mcp) = fixture();
    let err = mcp
        .set_link_direction(Parameters(SetLinkDirectionParams {
            workspace_id: "ghost-uuid".into(),
            direction: "in".into(),
        }))
        .await
        .expect_err("unknown workspace_id must error");
    let rendered = format!("{err:?}");
    assert!(
        rendered.contains("ghost-uuid"),
        "error must surface offending workspace_id, got `{rendered}`"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn refresh_peer_returns_invalid_params_for_unknown_workspace_id() {
    // L3-bis-2: unknown workspace_id surfaces as invalid_params
    // with the offending id in the data envelope, so MCP clients
    // can show a "no such peer" message without guessing.
    let (_dir, mcp) = fixture();
    let err = mcp
        .refresh_peer(Parameters(RefreshPeerParams {
            workspace_id: "ghost-uuid".into(),
        }))
        .await
        .expect_err("unknown workspace_id must error");
    let rendered = format!("{err:?}");
    assert!(
        rendered.contains("ghost-uuid"),
        "error must surface offending workspace_id, got `{rendered}`"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn refresh_peer_after_link_returns_ok_stats_envelope() {
    // L3-bis-2: link a peer (no source seeded, so 0 files) and
    // call refresh_peer. The Ok envelope must carry the
    // workspace_id + status="ok" + numeric counters.
    let (_dir, mcp) = fixture();
    let peer = tempfile::tempdir().unwrap();
    let link = mcp
        .link_workspace(Parameters(LinkWorkspaceParams {
            path: peer.path().to_string_lossy().into_owned(),
            direction: "in".into(),
            indexing_mode: Some("extract".into()),
        }))
        .await
        .expect("link ok");
    let workspace_id = body_text(&link)
        .split("\"workspace_id\": \"")
        .nth(1)
        .and_then(|tail| tail.split('"').next())
        .expect("workspace_id present")
        .to_string();

    let result = mcp
        .refresh_peer(Parameters(RefreshPeerParams {
            workspace_id: workspace_id.clone(),
        }))
        .await
        .expect("refresh_peer ok");
    let body = body_text(&result);
    assert!(body.contains(&workspace_id), "got `{body}`");
    assert!(body.contains("\"kind\": \"ok\""), "got `{body}`");
    assert!(body.contains("\"files_extracted\""), "got `{body}`");
}

#[tokio::test(flavor = "multi_thread")]
async fn module_lookup_returns_null_when_module_absent() {
    let (_dir, mcp) = fixture();
    let result = mcp
        .module_lookup(Parameters(ModuleLookupParams {
            module_fqdn: "no::such::module".into(),
            workspace_id: None,
        }))
        .await
        .expect("module_lookup ok");
    let body = body_text(&result);
    assert_eq!(body.trim(), "null");
}

#[tokio::test(flavor = "multi_thread")]
async fn resolve_cross_workspace_returns_empty_providers_on_fresh_index() {
    let (_dir, mcp) = fixture();
    let result = mcp
        .resolve_cross_workspace(Parameters(ResolveCrossWorkspaceParams {
            origin_module: "ws_b::lib".into(),
            origin_symbol: "Foo".into(),
        }))
        .await
        .expect("resolve_cross_workspace ok");
    let body = body_text(&result);
    assert!(body.contains("\"providers\": []"), "got `{body}`");
}

#[tokio::test(flavor = "multi_thread")]
async fn list_projects_returns_empty_array_on_fresh_index() {
    let (_dir, mcp) = fixture();
    let result = mcp.list_projects().await.expect("list_projects ok");
    let body = body_text(&result);
    assert!(body.contains("\"projects\""), "got `{body}`");
    assert!(body.contains("\"projects\": []"), "got `{body}`");
}

#[tokio::test(flavor = "multi_thread")]
async fn list_projects_surfaces_detected_projects_after_cold_start() {
    let (dir, mcp) = fixture();
    // Seed the fixture as a Rust project. cold_start runs
    // `discover_and_persist_projects` which picks up the manifest.
    std::fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    cold_start_workspace(&mcp, dir.path());

    let result = mcp.list_projects().await.expect("list_projects ok");
    let body = body_text(&result);
    assert!(
        body.contains("\"kind\": \"rust\""),
        "expected the fixture Rust project to appear, got `{body}`"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn list_groups_returns_empty_array_when_no_sxd() {
    let (_dir, mcp) = fixture();
    let result = mcp.list_groups().await.expect("list_groups ok");
    let body = body_text(&result);
    assert!(body.contains("\"groups\": []"), "got `{body}`");
}

#[tokio::test(flavor = "multi_thread")]
async fn list_groups_surfaces_declared_group_blocks() {
    let (dir, mcp) = fixture();
    std::fs::write(
        dir.path().join("standardoc.sxd"),
        "version \"0.1.0\"\n\
         \n\
         project \"core\" { path \"crates/core\" }\n\
         project \"cli\" { path \"crates/cli\" }\n\
         \n\
         group \"platform\" {\n\
           label \"Platform\"\n\
           members [\"core\" \"cli\"]\n\
         }\n",
    )
    .unwrap();

    let result = mcp.list_groups().await.expect("list_groups ok");
    let body = body_text(&result);
    assert!(body.contains("\"slug\": \"platform\""), "got `{body}`");
    assert!(body.contains("\"label\": \"Platform\""), "got `{body}`");
    assert!(body.contains("\"core\""), "got `{body}`");
    assert!(body.contains("\"cli\""), "got `{body}`");
}

#[tokio::test(flavor = "multi_thread")]
async fn project_for_file_returns_null_when_path_unregistered() {
    let (_dir, mcp) = fixture();
    let result = mcp
        .project_for_file(Parameters(ProjectForFileParams {
            path: "/no/such/file.rs".into(),
        }))
        .await
        .expect("project_for_file ok");
    let body = body_text(&result);
    assert_eq!(body.trim(), "null");
}

// --- IR-4-f follow-up: find_call_sites MCP tool ---

/// Write a Rust fixture under the workspace root so cold_start ends
/// up walking it and populating `call_sites` via the real extractor
/// + storage path. Cheaper than wiring around `pool()`'s pub(crate)
/// visibility, and validates the full IR-4-b → IR-4-f pipeline in
/// one shot.
fn seed_rust_call_sites(root: &Path) {
    let src_dir = root.join("src");
    std::fs::create_dir_all(&src_dir).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    // `caller_a` calls `tauri_invoke` (resolves locally) and
    // `foo`; `caller_b` also calls `tauri_invoke`; `caller_c`
    // calls a multi-segment member-access expression. Match the
    // call_text patterns the test queries below expect.
    std::fs::write(
        src_dir.join("lib.rs"),
        r"
                fn tauri_invoke() {}
                fn foo() {}
                fn caller_a() { tauri_invoke(); foo(); }
                fn caller_b() { tauri_invoke(); }
                fn caller_c() { M.api.create(); }
            ",
    )
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn find_call_sites_returns_indexing_in_progress_when_not_ready() {
    let (_dir, mcp) = fixture();
    let result = mcp
        .find_call_sites(Parameters(FindCallSitesParams {
            from_fqdn: None,
            callee_text: None,
            callee_pattern: None,
            limit: None,
        }))
        .await
        .expect("tool returns Ok with friendly degradation");
    assert!(body_text(&result).contains("Workspace indexing in progress"));
}

#[tokio::test(flavor = "multi_thread")]
async fn find_call_sites_no_filter_returns_all_extracted_rows() {
    // E2E pipeline check — extractor populates call_sites, storage
    // persists them, the MCP tool reads them back.
    let (dir, mcp) = fixture();
    seed_rust_call_sites(dir.path());
    cold_start_workspace(&mcp, dir.path());
    let result = mcp
        .find_call_sites(Parameters(FindCallSitesParams {
            from_fqdn: None,
            callee_text: None,
            callee_pattern: None,
            limit: None,
        }))
        .await
        .unwrap();
    let body = body_text(&result);
    let arr: serde_json::Value = serde_json::from_str(&body).unwrap();
    // 4 calls in the fixture: caller_a→tauri_invoke, caller_a→foo,
    // caller_b→tauri_invoke, caller_c→M.api.create.
    assert_eq!(
        arr["call_sites"].as_array().unwrap().len(),
        4,
        "expected 4 extracted call_sites, got `{body}`"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn find_call_sites_filter_by_callee_text_narrows_to_matching_records() {
    let (dir, mcp) = fixture();
    seed_rust_call_sites(dir.path());
    cold_start_workspace(&mcp, dir.path());
    let result = mcp
        .find_call_sites(Parameters(FindCallSitesParams {
            from_fqdn: None,
            callee_text: Some("tauri_invoke".into()),
            callee_pattern: None,
            limit: None,
        }))
        .await
        .unwrap();
    let body = body_text(&result);
    let arr: serde_json::Value = serde_json::from_str(&body).unwrap();
    let rows = arr["call_sites"].as_array().unwrap();
    assert_eq!(
        rows.len(),
        2,
        "two tauri_invoke calls in the fixture, got `{body}`"
    );
    for row in rows {
        assert_eq!(row["callee_text"].as_str(), Some("tauri_invoke"));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn find_call_sites_filter_by_from_fqdn_returns_calls_of_one_caller() {
    let (dir, mcp) = fixture();
    seed_rust_call_sites(dir.path());
    cold_start_workspace(&mcp, dir.path());
    // The extractor stamps `from_fqdn` as the crate-relative FQDN of
    // the enclosing fn. For our fixture: `fixture::caller_a`.
    let result = mcp
        .find_call_sites(Parameters(FindCallSitesParams {
            from_fqdn: Some("fixture::caller_a".into()),
            callee_text: None,
            callee_pattern: None,
            limit: None,
        }))
        .await
        .unwrap();
    let body = body_text(&result);
    let arr: serde_json::Value = serde_json::from_str(&body).unwrap();
    let rows = arr["call_sites"].as_array().unwrap();
    assert_eq!(
        rows.len(),
        2,
        "caller_a calls both tauri_invoke + foo, got `{body}`"
    );
    for row in rows {
        assert_eq!(row["from_fqdn"].as_str(), Some("fixture::caller_a"));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn find_call_sites_filter_by_callee_pattern_matches_glob() {
    let (dir, mcp) = fixture();
    seed_rust_call_sites(dir.path());
    cold_start_workspace(&mcp, dir.path());
    // `M.api.create` is the only multi-dotted callee in the fixture.
    let result = mcp
        .find_call_sites(Parameters(FindCallSitesParams {
            from_fqdn: None,
            callee_text: None,
            callee_pattern: Some("M.api.*".into()),
            limit: None,
        }))
        .await
        .unwrap();
    let body = body_text(&result);
    let arr: serde_json::Value = serde_json::from_str(&body).unwrap();
    let rows = arr["call_sites"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "only M.api.create matches the glob");
    assert_eq!(rows[0]["callee_text"].as_str(), Some("M.api.create"));
}

#[tokio::test(flavor = "multi_thread")]
async fn find_call_sites_empty_string_filter_treated_as_unset() {
    // MCP callers often serialize `Option::None` as `""` — the
    // server-side `non_empty` normalises it back so a vacuous
    // filter doesn't silently constrain the result set.
    let (dir, mcp) = fixture();
    seed_rust_call_sites(dir.path());
    cold_start_workspace(&mcp, dir.path());
    let result = mcp
        .find_call_sites(Parameters(FindCallSitesParams {
            from_fqdn: Some(String::new()),
            callee_text: Some("   ".into()),
            callee_pattern: None,
            limit: None,
        }))
        .await
        .unwrap();
    let body = body_text(&result);
    let arr: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(
        arr["call_sites"].as_array().unwrap().len(),
        4,
        "empty / whitespace filters must read as no filter, got `{body}`"
    );
}

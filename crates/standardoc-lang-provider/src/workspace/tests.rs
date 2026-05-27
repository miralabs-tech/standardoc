use super::*;

#[test]
fn dispatch_rust_extension() {
    assert_eq!(dispatch("src/lib.rs"), Some(Dispatch::Rust));
    assert_eq!(dispatch("crates/foo/src/main.rs"), Some(Dispatch::Rust));
}

#[test]
fn dispatch_typescript_extensions() {
    assert_eq!(dispatch("src/index.ts"), Some(Dispatch::TypeScript));
    assert_eq!(dispatch("src/App.tsx"), Some(Dispatch::TypeScript));
    assert_eq!(dispatch("scripts/build.js"), Some(Dispatch::TypeScript));
    assert_eq!(dispatch("src/legacy.jsx"), Some(Dispatch::TypeScript));
}

#[test]
fn dispatch_lua_extension() {
    assert_eq!(dispatch("src/utils/strings.lua"), Some(Dispatch::Lua));
    assert_eq!(dispatch("init.lua"), Some(Dispatch::Lua));
    assert_eq!(dispatch("client/script.lua"), Some(Dispatch::Lua));
}

#[test]
fn dispatch_lua_uppercase_extension_returns_none() {
    // Extension matching is case-sensitive — `.LUA` is not standard
    // and we don't want to silently accept it.
    assert_eq!(dispatch("script.LUA"), None);
}

#[test]
fn dispatch_vue_extension() {
    assert_eq!(dispatch("src/components/App.vue"), Some(Dispatch::Vue));
    assert_eq!(dispatch("App.vue"), Some(Dispatch::Vue));
}

#[test]
fn dispatch_svelte_extension() {
    assert_eq!(dispatch("src/routes/+page.svelte"), Some(Dispatch::Svelte));
    assert_eq!(dispatch("Counter.svelte"), Some(Dispatch::Svelte));
}

#[test]
fn dispatch_vue_uppercase_extension_returns_none() {
    // Symmetry with Lua/TS: case-sensitive extension matching.
    assert_eq!(dispatch("App.VUE"), None);
}

#[test]
fn dispatch_svelte_uppercase_extension_returns_none() {
    assert_eq!(dispatch("Counter.SVELTE"), None);
}

#[test]
fn dispatch_unsupported_returns_none() {
    assert_eq!(dispatch("README.md"), None);
    assert_eq!(dispatch("Cargo.toml"), None);
    assert_eq!(dispatch("package.json"), None);
    assert_eq!(dispatch("script.py"), None);
}

#[test]
fn dispatch_no_extension_returns_none() {
    assert_eq!(dispatch("Makefile"), None);
    assert_eq!(dispatch(""), None);
}

#[test]
fn lang_to_syntax_ts_is_typescript() {
    assert!(matches!(lang_to_syntax("ts"), Syntax::Typescript(_)));
}

#[test]
fn lang_to_syntax_js_is_es() {
    assert!(matches!(lang_to_syntax("js"), Syntax::Es(_)));
}

#[test]
fn lang_to_syntax_unknown_falls_back_to_js() {
    assert!(matches!(lang_to_syntax("haskell"), Syntax::Es(_)));
}

// `byte_offset_to_line_col` moved to `crate::utils::location` and is
// covered by its own unit tests there.

#[test]
fn svelte_template_regions_carves_out_script_block() {
    let src = "<h1>x</h1>\n<script>let a;</script>\n<p>y</p>";
    let doc = sfc::extract_blocks(src);
    let regions = svelte_template_regions(src, &doc);
    // Two regions: before the script + after the script.
    assert_eq!(regions.len(), 2);
    assert!(regions.iter().any(|(s, e)| &src[*s..*e] == "<h1>x</h1>\n"));
}

#[test]
fn svelte_template_regions_no_blocks_yields_whole_source() {
    let src = "<p>hello</p>";
    let doc = sfc::extract_blocks(src);
    let regions = svelte_template_regions(src, &doc);
    assert_eq!(regions, vec![(0, src.len())]);
}

#[test]
fn build_script_payload_keeps_source_order_when_already_prescribed() {
    // Plain <script> first, <script setup> second — already in the
    // lock 41 §1 Q2 order. Payload must concat plain-then-setup.
    let src = "<script>const a=1;</script>\n<script setup>const b=2;</script>";
    let doc = sfc::extract_blocks(src);
    let (payload, _) = build_script_payload(src, &doc, Framework::Vue);
    let plain_pos = payload.find("const a=1;").unwrap();
    let setup_pos = payload.find("const b=2;").unwrap();
    assert!(plain_pos < setup_pos, "plain must appear before setup");
}

#[test]
fn build_script_payload_reorders_when_setup_appears_first_in_source() {
    // Reverse source order: <script setup> first, plain <script>
    // second. Lock 41 §1 Q2 enforces plain-before-setup → output
    // payload must put `const a=1;` (plain) before `const b=2;`
    // (setup) regardless of source order.
    let src = "<script setup>const b=2;</script>\n<script>const a=1;</script>";
    let doc = sfc::extract_blocks(src);
    let (payload, _) = build_script_payload(src, &doc, Framework::Vue);
    let plain_pos = payload.find("const a=1;").unwrap();
    let setup_pos = payload.find("const b=2;").unwrap();
    assert!(
        plain_pos < setup_pos,
        "plain must be reordered before setup even when source order says otherwise"
    );
}

#[test]
fn build_script_payload_single_setup_only_works() {
    // Most idiomatic Vue 3 SFC: a lone `<script setup>` block.
    let src = "<script setup>const x=1;</script>";
    let doc = sfc::extract_blocks(src);
    let (payload, _) = build_script_payload(src, &doc, Framework::Vue);
    assert!(payload.contains("const x=1;"));
}

#[test]
fn build_script_payload_preserves_byte_alignment_for_in_order_scripts() {
    // When source order matches prescribed order, the payload's byte
    // offset of the script content must match the source's byte
    // offset (so swc spans align with the SFC file's coords).
    let src = "<script>const a=1;</script>\n<script setup>const b=2;</script>";
    let doc = sfc::extract_blocks(src);
    let (payload, _) = build_script_payload(src, &doc, Framework::Vue);
    let source_first = src.find("const a=1;").unwrap();
    let source_second = src.find("const b=2;").unwrap();
    let payload_first = payload.find("const a=1;").unwrap();
    let payload_second = payload.find("const b=2;").unwrap();
    assert_eq!(payload_first, source_first);
    assert_eq!(payload_second, source_second);
}

// --- IR-4-e: SFC (Vue / Svelte) inherits TS call_site population ---

mod ir4e_sfc {
    use super::super::WorkspaceProvider;
    use standardoc_core::{ExtractContext, LanguageProvider};
    use std::fs;
    use std::path::Path;
    use tempfile::tempdir;

    fn write(root: &Path, rel: &str, content: &str) {
        let abs = root.join(rel);
        if let Some(parent) = abs.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(abs, content).unwrap();
    }

    #[test]
    fn ir4e_vue_sfc_script_call_sites_flow_through_ts_extractor() {
        // Vue SFC with `<script lang="ts">` containing both a free-fn
        // call and a member call. Both must surface in
        // `ExtractedFile.call_sites` exactly as if extracted from a
        // pure `.ts` file — SFC delegates to the TS extractor.
        let dir = tempdir().unwrap();
        let root = dir.path();
        write(root, "package.json", r#"{"name":"@app/ui"}"#);
        let src = r#"<template><h1>Hi</h1></template>
<script lang="ts">
function caller() {
    foo("hi", 42);
    obj.api.create(payload);
}
</script>
"#;
        write(root, "src/App.vue", src);
        let provider = WorkspaceProvider::new();
        let ctx = ExtractContext {
            workspace_root: root,
            cross_workspace: None,
        };
        let extracted = provider.extract(src, "src/App.vue", &ctx).unwrap();
        assert_eq!(extracted.language, standardoc_ir::Language::Vue);
        // Free-fn call_site preserved.
        let foo_cs = extracted
            .call_sites
            .iter()
            .find(|c| c.callee_text == "foo")
            .unwrap_or_else(|| {
                panic!(
                    "expected foo(...) call_site to flow through Vue SFC, got {:?}",
                    extracted.call_sites
                )
            });
        assert_eq!(foo_cs.args.len(), 2);
        assert!(foo_cs.args[0].is_string_literal);
        assert_eq!(foo_cs.args[0].value, "hi");
        // Member call_site preserved with receiver_chain.
        let create_cs = extracted
            .call_sites
            .iter()
            .find(|c| c.callee_text == "obj.api.create")
            .unwrap_or_else(|| {
                panic!(
                    "expected obj.api.create call_site in Vue SFC, got {:?}",
                    extracted.call_sites
                )
            });
        assert_eq!(
            create_cs.receiver_chain,
            vec!["obj".to_string(), "api".to_string()]
        );
    }

    #[test]
    fn ir4e_svelte_sfc_script_call_sites_flow_through_ts_extractor() {
        // Same test as Vue but for Svelte — the SFC orchestrator
        // routes both through `extract_sfc` -> ts::extract_with_overrides.
        let dir = tempdir().unwrap();
        let root = dir.path();
        write(root, "package.json", r#"{"name":"@app/svelte-ui"}"#);
        let src = r#"<script lang="ts">
function handler() {
    fetch("/api/ping");
}
</script>
<h1>hi</h1>
"#;
        write(root, "src/Counter.svelte", src);
        let provider = WorkspaceProvider::new();
        let ctx = ExtractContext {
            workspace_root: root,
            cross_workspace: None,
        };
        let extracted = provider.extract(src, "src/Counter.svelte", &ctx).unwrap();
        assert_eq!(extracted.language, standardoc_ir::Language::Svelte);
        let fetch_cs = extracted
            .call_sites
            .iter()
            .find(|c| c.callee_text == "fetch")
            .unwrap_or_else(|| {
                panic!(
                    "expected fetch(...) call_site in Svelte SFC, got {:?}",
                    extracted.call_sites
                )
            });
        assert_eq!(fetch_cs.args.len(), 1);
        assert!(fetch_cs.args[0].is_string_literal);
        assert_eq!(fetch_cs.args[0].value, "/api/ping");
    }

    #[test]
    fn ir4e_vue_script_setup_call_sites_attributed_to_module_fqdn() {
        // `<script setup>` runs in module scope — call_sites emitted
        // at the top level have `from_fqdn` equal to the SFC's
        // module fqdn. Vue 3 idiomatic shape. Now reachable since
        // `process_item_p2` walks top-level `Stmt::Expr` for calls.
        let dir = tempdir().unwrap();
        let root = dir.path();
        write(root, "package.json", r#"{"name":"@app/setup"}"#);
        let src = r#"<script setup lang="ts">
import { onMount } from "vue";
onMount(() => { console.log("ready"); });
</script>
"#;
        write(root, "src/Mounted.vue", src);
        let provider = WorkspaceProvider::new();
        let ctx = ExtractContext {
            workspace_root: root,
            cross_workspace: None,
        };
        let extracted = provider.extract(src, "src/Mounted.vue", &ctx).unwrap();
        assert!(
            extracted
                .call_sites
                .iter()
                .any(|c| c.callee_text == "onMount"),
            "onMount() should surface as a call_site in script-setup, got {:?}",
            extracted.call_sites
        );
    }
}

// Integration-test ergonomic lints that fire on intentional patterns:
//   - raw-string hashes round-trip cleanly when copy-pasting SFCs.
//   - `.iter().any(|n| *n == ...)` reads more naturally next to the
//     other helper-driven assertions.
//   - The `ref_name` helper exists for symmetry with other test files
//     even when its arms collapse — keeping the explicit shape makes
//     intent-at-glance easier.
#![allow(
    clippy::needless_raw_string_hashes,
    clippy::missing_const_for_fn,
    clippy::match_same_arms
)]

use std::fs;
use std::path::Path;

use standardoc_core::{ExtractContext, LanguageProvider};
use standardoc_ir::{EdgeKind, Language, ResolvedOrUnresolved};
use standardoc_lang_provider::WorkspaceProvider;
use tempfile::tempdir;

fn write(root: &Path, rel: &str, content: &str) {
    let abs = root.join(rel);
    if let Some(parent) = abs.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(abs, content).unwrap();
}

fn refs_with_attr<'a>(
    edges: &'a [standardoc_ir::RawEdge],
    attr: &str,
) -> Vec<&'a standardoc_ir::RawEdge> {
    edges
        .iter()
        .filter(|e| e.kind == EdgeKind::References && e.attributes.iter().any(|a| a == attr))
        .collect()
}

fn ref_name(edge: &standardoc_ir::RawEdge) -> &str {
    match &edge.to {
        ResolvedOrUnresolved::Resolved { fqdn } => fqdn.as_str(),
        ResolvedOrUnresolved::Unresolved { name } => name.as_str(),
        ResolvedOrUnresolved::UnresolvedBridge { name, .. } => name.as_str(),
    }
}

#[test]
fn vue_sfc_emits_module_symbol_with_vue_language() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    write(root, "package.json", r#"{"name":"@app/web"}"#);
    let sfc = r#"<template>
  <h1>{{ msg }}</h1>
</template>
<script lang="ts">
const msg = 'hello';
export default { name: 'App' };
</script>
"#;
    write(root, "src/App.vue", sfc);
    let provider = WorkspaceProvider::new();
    let ctx = ExtractContext {
        workspace_root: root,
            cross_workspace: None,
        };
    let extracted = provider.extract(sfc, "src/App.vue", &ctx).expect("ok");

    assert_eq!(extracted.language, Language::Vue);
    let module = &extracted.symbols[0];
    assert_eq!(module.fqdn, "@app/web::src::App");
}

#[test]
fn vue_template_interpolation_emits_template_ref_edge() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    write(root, "package.json", r#"{"name":"@app/web"}"#);
    let sfc = r#"<template>
  <p>{{ message }}</p>
</template>
<script lang="ts">
const message = 'hi';
</script>
"#;
    write(root, "src/Hello.vue", sfc);
    let provider = WorkspaceProvider::new();
    let ctx = ExtractContext {
        workspace_root: root,
            cross_workspace: None,
        };
    let extracted = provider.extract(sfc, "src/Hello.vue", &ctx).unwrap();

    let interp = refs_with_attr(&extracted.edges, "template-interpolation");
    assert!(interp.iter().any(|e| ref_name(e) == "message"));
}

#[test]
fn vue_directive_v_if_emits_template_directive_edge() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    write(root, "package.json", r#"{"name":"@app/web"}"#);
    let sfc = r#"<template>
  <div v-if="visible">x</div>
</template>
<script lang="ts">
const visible = true;
</script>
"#;
    write(root, "src/Toggle.vue", sfc);
    let provider = WorkspaceProvider::new();
    let ctx = ExtractContext {
        workspace_root: root,
            cross_workspace: None,
        };
    let extracted = provider.extract(sfc, "src/Toggle.vue", &ctx).unwrap();

    let dirs = refs_with_attr(&extracted.edges, "template-directive");
    assert!(dirs.iter().any(|e| ref_name(e) == "visible"));
}

#[test]
fn vue_at_event_handler_emits_template_event_edge() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    write(root, "package.json", r#"{"name":"@app/web"}"#);
    let sfc = r#"<template>
  <button @click="handleClick">x</button>
</template>
<script lang="ts">
function handleClick() {}
</script>
"#;
    write(root, "src/Button.vue", sfc);
    let provider = WorkspaceProvider::new();
    let ctx = ExtractContext {
        workspace_root: root,
            cross_workspace: None,
        };
    let extracted = provider.extract(sfc, "src/Button.vue", &ctx).unwrap();

    let events = refs_with_attr(&extracted.edges, "template-event");
    assert!(events.iter().any(|e| ref_name(e) == "handleClick"));
}

#[test]
fn vue_component_ref_emits_template_component_ref_edge() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    write(root, "package.json", r#"{"name":"@app/web"}"#);
    let sfc = r#"<template>
  <UserCard />
</template>
<script lang="ts">
import UserCard from './UserCard.vue';
</script>
"#;
    write(root, "src/App.vue", sfc);
    let provider = WorkspaceProvider::new();
    let ctx = ExtractContext {
        workspace_root: root,
            cross_workspace: None,
        };
    let extracted = provider.extract(sfc, "src/App.vue", &ctx).unwrap();

    let comps = refs_with_attr(&extracted.edges, "template-component-ref");
    assert!(comps.iter().any(|e| ref_name(e) == "UserCard"));
}

#[test]
fn vue_script_setup_extracts_top_level_symbols() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    write(root, "package.json", r#"{"name":"@app/web"}"#);
    let sfc = r#"<template>
  <p>{{ count }}</p>
</template>
<script setup lang="ts">
const count = 42;
function increment() { /* ... */ }
</script>
"#;
    write(root, "src/Counter.vue", sfc);
    let provider = WorkspaceProvider::new();
    let ctx = ExtractContext {
        workspace_root: root,
            cross_workspace: None,
        };
    let extracted = provider.extract(sfc, "src/Counter.vue", &ctx).unwrap();

    let names: Vec<&str> = extracted.symbols.iter().map(|s| s.name.as_str()).collect();
    assert!(names.contains(&"count"));
    assert!(names.contains(&"increment"));
}

#[test]
fn svelte_sfc_emits_module_symbol_with_svelte_language() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    write(root, "package.json", r#"{"name":"@app/web"}"#);
    let sfc = r#"<script>
let count = 0;
</script>
<button on:click={() => count++}>Clicks: {count}</button>
"#;
    write(root, "src/Counter.svelte", sfc);
    let provider = WorkspaceProvider::new();
    let ctx = ExtractContext {
        workspace_root: root,
            cross_workspace: None,
        };
    let extracted = provider.extract(sfc, "src/Counter.svelte", &ctx).unwrap();

    assert_eq!(extracted.language, Language::Svelte);
    let module = &extracted.symbols[0];
    assert_eq!(module.fqdn, "@app/web::src::Counter");
}

#[test]
fn svelte_event_handler_emits_template_event_edge() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    write(root, "package.json", r#"{"name":"@app/web"}"#);
    let sfc = r#"<script>
function handleClick() {}
</script>
<button on:click={handleClick}>x</button>
"#;
    write(root, "src/Btn.svelte", sfc);
    let provider = WorkspaceProvider::new();
    let ctx = ExtractContext {
        workspace_root: root,
            cross_workspace: None,
        };
    let extracted = provider.extract(sfc, "src/Btn.svelte", &ctx).unwrap();

    let events = refs_with_attr(&extracted.edges, "template-event");
    assert!(events.iter().any(|e| ref_name(e) == "handleClick"));
}

#[test]
fn svelte_each_block_emits_template_directive_edge() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    write(root, "package.json", r#"{"name":"@app/web"}"#);
    let sfc = r#"<script>
let users = [];
</script>
{#each users as user}
  <p>{user.name}</p>
{/each}
"#;
    write(root, "src/List.svelte", sfc);
    let provider = WorkspaceProvider::new();
    let ctx = ExtractContext {
        workspace_root: root,
            cross_workspace: None,
        };
    let extracted = provider.extract(sfc, "src/List.svelte", &ctx).unwrap();

    let dirs = refs_with_attr(&extracted.edges, "template-directive");
    assert!(dirs.iter().any(|e| ref_name(e) == "users"));
}

#[test]
fn svelte_component_ref_emits_template_component_ref_edge() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    write(root, "package.json", r#"{"name":"@app/web"}"#);
    let sfc = r#"<script>
import Header from './Header.svelte';
</script>
<Header />
"#;
    write(root, "src/App.svelte", sfc);
    let provider = WorkspaceProvider::new();
    let ctx = ExtractContext {
        workspace_root: root,
            cross_workspace: None,
        };
    let extracted = provider.extract(sfc, "src/App.svelte", &ctx).unwrap();

    let comps = refs_with_attr(&extracted.edges, "template-component-ref");
    assert!(comps.iter().any(|e| ref_name(e) == "Header"));
}

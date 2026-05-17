use std::path::Path;

use standardoc_core::{ExtractContext, ExtractError, LanguageProvider};
use standardoc_ir::{
    BuiltinEntry, BuiltinTier, EdgeKind, ExtractedFile, Language, RawEdge, ResolvedOrUnresolved,
    Site,
};
use swc_core::ecma::parser::{EsSyntax, Syntax, TsSyntax};

use crate::builtins::global as global_builtin_registry;
use crate::lua::LuaProvider;
use crate::rust::RustProvider;
use crate::sfc::{self, SfcDocument, pad_until_byte_offset};
use crate::template::{self, TemplateAttribute, TemplateRef};
use crate::ts::TsProvider;
use crate::utils::byte_offset_to_line_col;

#[derive(Debug, Default)]
pub struct WorkspaceProvider {
    rust: RustProvider,
    ts: TsProvider,
    lua: LuaProvider,
}

impl WorkspaceProvider {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn extract_sfc(
        &self,
        content: &str,
        path: &str,
        ctx: &ExtractContext<'_>,
        framework: Framework,
    ) -> Result<ExtractedFile, ExtractError> {
        let doc = sfc::extract_blocks(content);
        let (script_payload, syntax) = build_script_payload(content, &doc, framework);
        let mut extracted = self.ts.extract_with_overrides(
            &script_payload,
            path,
            ctx,
            Some(syntax),
            Some(framework.ir_language()),
        )?;
        // Append template-extracted REFERENCES edges. The first symbol on
        // the extracted file is the Module symbol — we use its FQDN as
        // the `from_fqdn` for every template ref so they all hang off the
        // SFC-as-component identity.
        let module_fqdn = extracted
            .symbols
            .first()
            .map(|s| s.fqdn.clone())
            .unwrap_or_default();
        let template_refs = collect_template_refs(content, &doc, framework);
        for r in template_refs {
            extracted
                .edges
                .push(template_ref_to_edge(r, content, path, &module_fqdn));
        }
        Ok(extracted)
    }
}

#[derive(Debug, Clone, Copy)]
enum Framework {
    Vue,
    Svelte,
}

impl Framework {
    const fn ir_language(self) -> Language {
        match self {
            Self::Vue => Language::Vue,
            Self::Svelte => Language::Svelte,
        }
    }

    /// Default script `lang` when the SFC author omitted the attribute.
    /// Lock 41 §1 Q8: Vue defaults to TS, Svelte defaults to JS.
    const fn default_lang(self) -> &'static str {
        match self {
            Self::Vue => "ts",
            Self::Svelte => "js",
        }
    }
}

impl LanguageProvider for WorkspaceProvider {
    fn extract(
        &self,
        content: &str,
        path: &str,
        ctx: &ExtractContext<'_>,
    ) -> Result<ExtractedFile, ExtractError> {
        match dispatch(path) {
            Some(Dispatch::Rust) => self.rust.extract(content, path, ctx),
            Some(Dispatch::TypeScript) => self.ts.extract(content, path, ctx),
            Some(Dispatch::Lua) => self.lua.extract(content, path, ctx),
            Some(Dispatch::Vue) => self.extract_sfc(content, path, ctx, Framework::Vue),
            Some(Dispatch::Svelte) => self.extract_sfc(content, path, ctx, Framework::Svelte),
            None => Err(ExtractError::UnsupportedLanguage { file: path.into() }),
        }
    }

    /// Edge-tier entries flattened across every registered language —
    /// the cold-start seeder turns each into a synthetic `RawSymbol`
    /// row so resolver-emitted edges to `<builtin>::<lang>::<name>`
    /// land on a real `symbols.id` instead of unresolved canonicals.
    fn edge_builtins(&self) -> Vec<BuiltinEntry> {
        let reg = global_builtin_registry();
        let mut out: Vec<BuiltinEntry> = Vec::new();
        for entries in reg.by_language.values() {
            out.extend(
                entries
                    .iter()
                    .filter(|e| e.tier == BuiltinTier::Edge)
                    .cloned(),
            );
        }
        out.extend(
            reg.user_extensions
                .iter()
                .filter(|e| e.tier == BuiltinTier::Edge)
                .cloned(),
        );
        out
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Dispatch {
    Rust,
    TypeScript,
    Lua,
    Vue,
    Svelte,
}

fn dispatch(path: &str) -> Option<Dispatch> {
    let ext = Path::new(path).extension()?.to_str()?;
    match ext {
        "rs" => Some(Dispatch::Rust),
        "ts" | "tsx" | "js" | "jsx" => Some(Dispatch::TypeScript),
        "lua" => Some(Dispatch::Lua),
        "vue" => Some(Dispatch::Vue),
        "svelte" => Some(Dispatch::Svelte),
        _ => None,
    }
}

/// Concatenates every script block, padding the prefix and inter-block
/// gaps with whitespace so the resulting payload's byte offsets line up
/// 1:1 with the original SFC source. Returns the payload + the swc
/// syntax derived from the first script's `lang` attribute (or the
/// framework default).
///
/// Lock 41 §1 Q2: `<script setup>` always lands AFTER plain `<script>`
/// in the merged payload — Vue 3 semantics require `defineProps` etc.
/// to see imports declared at module scope. We stable-sort scripts by
/// the `is_script_setup` flag so plain blocks come first regardless of
/// source order. Byte alignment is preserved when source order matches
/// the prescribed order (the overwhelmingly common case); when the
/// user writes `<script setup>` before `<script>` in source, the
/// reordered second block's swc spans drift by the inter-block byte
/// gap — accepted edge case (cohérent feedback_scope_graph_not_lsp).
fn build_script_payload(
    content: &str,
    doc: &SfcDocument,
    framework: Framework,
) -> (String, Syntax) {
    let mut scripts: Vec<&sfc::SfcBlock> = doc.scripts.iter().collect();
    scripts.sort_by_key(|s| s.is_script_setup());

    let mut payload = String::new();
    for script in &scripts {
        pad_until_byte_offset(&mut payload, script.content_start, content);
        payload.push_str(&content[script.content_start..script.content_end]);
    }
    let lang = scripts
        .iter()
        .find_map(|s| s.lang.as_deref())
        .unwrap_or_else(|| framework.default_lang());
    (payload, lang_to_syntax(lang))
}

fn lang_to_syntax(lang: &str) -> Syntax {
    match lang {
        "ts" | "typescript" => Syntax::Typescript(TsSyntax {
            tsx: false,
            decorators: false,
            dts: false,
            no_early_errors: true,
            disallow_ambiguous_jsx_like: false,
        }),
        "tsx" => Syntax::Typescript(TsSyntax {
            tsx: true,
            decorators: false,
            dts: false,
            no_early_errors: true,
            disallow_ambiguous_jsx_like: false,
        }),
        "jsx" => Syntax::Es(EsSyntax {
            jsx: true,
            ..Default::default()
        }),
        // "js" / "javascript" / unknown — default to plain JS.
        _ => Syntax::Es(EsSyntax::default()),
    }
}

fn collect_template_refs(
    content: &str,
    doc: &SfcDocument,
    framework: Framework,
) -> Vec<TemplateRef> {
    let mut sink: Vec<TemplateRef> = Vec::new();
    match framework {
        Framework::Vue => {
            if let Some(t) = &doc.template {
                let body = &content[t.content_start..t.content_end];
                template::vue::parse(body, t.content_start, &mut sink);
            }
        }
        Framework::Svelte => {
            // Svelte doesn't wrap its template in a `<template>` block —
            // the "template" is the whole SFC source minus the script
            // and style blocks. Carve those regions out before parsing.
            let template_regions = svelte_template_regions(content, doc);
            for (start, end) in template_regions {
                template::svelte::parse(&content[start..end], start, &mut sink);
            }
        }
    }
    sink
}

/// Returns `(start, end)` byte ranges of the regions of `content` that
/// belong to the Svelte template (everything that's NOT inside a
/// `<script>` or `<style>` block).
fn svelte_template_regions(content: &str, doc: &SfcDocument) -> Vec<(usize, usize)> {
    let mut blocked: Vec<(usize, usize)> = doc
        .scripts
        .iter()
        .chain(doc.styles.iter())
        .map(|b| {
            (
                block_outer_start(content, b.content_start),
                block_outer_end(content, b.content_end),
            )
        })
        .collect();
    blocked.sort_by_key(|&(s, _)| s);
    let mut regions = Vec::new();
    let mut cursor = 0usize;
    for (s, e) in blocked {
        if cursor < s {
            regions.push((cursor, s));
        }
        cursor = e;
    }
    if cursor < content.len() {
        regions.push((cursor, content.len()));
    }
    regions
}

/// Walks backward from `content_start` to the matching `<` of the
/// opening tag — needed because `SfcBlock.content_start` points just
/// past `>` and we want to exclude the whole `<script ...>` from the
/// Svelte template region.
fn block_outer_start(content: &str, content_start: usize) -> usize {
    let bytes = content.as_bytes();
    let mut i = content_start.saturating_sub(1);
    while i > 0 {
        if bytes[i] == b'<' {
            return i;
        }
        i -= 1;
    }
    0
}

/// Walks forward from `content_end` to the byte just past the closing
/// `</tag>`'s `>`.
fn block_outer_end(content: &str, content_end: usize) -> usize {
    let bytes = content.as_bytes();
    let mut i = content_end;
    while i < bytes.len() {
        if bytes[i] == b'>' {
            return i + 1;
        }
        i += 1;
    }
    bytes.len()
}

/// Day-1 best-effort: emit each [`TemplateRef`] as an Unresolved
/// REFERENCES edge with the bare local identifier name. The
/// `promote_unresolved_batch` pass will lift the edge if any symbol's
/// fqdn happens to match the bare name (rare). Resolution against the
/// SFC's import alias table is a beta.2 follow-up.
fn template_ref_to_edge(r: TemplateRef, content: &str, path: &str, module_fqdn: &str) -> RawEdge {
    let (line, col) = byte_offset_to_line_col(content, r.byte_offset);
    let to = ResolvedOrUnresolved::Unresolved { name: r.name };
    let confidence = to.default_confidence();
    RawEdge {
        from_fqdn: module_fqdn.to_string(),
        kind: EdgeKind::References,
        to,
        sites: vec![Site {
            file: path.to_string(),
            line,
            col,
        }],
        attributes: vec![template_attr_to_slug(r.attribute).to_string()],
        confidence,
    }
}

const fn template_attr_to_slug(attr: TemplateAttribute) -> &'static str {
    attr.as_str()
}

#[cfg(test)]
mod tests {
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
            let ctx = ExtractContext { workspace_root: root };
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
            let ctx = ExtractContext { workspace_root: root };
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
        #[ignore = "TS walker doesn't visit top-level Stmt::Expr (only Stmt::Decl). \
                    Vue 3 script-setup idiom of top-level call statements is therefore \
                    invisible to call_site emission. Pre-existing limitation; activates \
                    when top-level expression walking is wired up (separate change)."]
        fn ir4e_vue_script_setup_call_sites_attributed_to_module_fqdn() {
            // `<script setup>` runs in module scope — call_sites emitted
            // at the top level would have `from_fqdn` equal to the SFC's
            // module fqdn. Vue 3 idiomatic shape. Ignored because the TS
            // walker currently skips top-level expression statements.
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
            let ctx = ExtractContext { workspace_root: root };
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
}

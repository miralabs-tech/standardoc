use std::path::Path;

use standardoc_core::{ExtractContext, ExtractError, LanguageProvider};
use standardoc_ir::{
    EdgeKind, ExtractedFile, Language, RawEdge, ResolvedOrUnresolved, Site,
};
use swc_core::ecma::parser::{EsSyntax, Syntax, TsSyntax};

use crate::lua::LuaProvider;
use crate::rust::RustProvider;
use crate::sfc::{self, SfcDocument, pad_until_byte_offset};
use crate::template::{self, TemplateAttribute, TemplateRef};
use crate::ts::TsProvider;

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
            extracted.edges.push(template_ref_to_edge(r, content, path, &module_fqdn));
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
fn build_script_payload(
    content: &str,
    doc: &SfcDocument,
    framework: Framework,
) -> (String, Syntax) {
    let mut payload = String::new();
    for script in &doc.scripts {
        pad_until_byte_offset(&mut payload, script.content_start, content);
        payload.push_str(&content[script.content_start..script.content_end]);
    }
    let lang = doc
        .scripts
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
        .map(|b| (block_outer_start(content, b.content_start), block_outer_end(content, b.content_end)))
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
fn template_ref_to_edge(
    r: TemplateRef,
    content: &str,
    path: &str,
    module_fqdn: &str,
) -> RawEdge {
    let (line, col) = byte_offset_to_line_col(content, r.byte_offset);
    RawEdge {
        from_fqdn: module_fqdn.to_string(),
        kind: EdgeKind::References,
        to: ResolvedOrUnresolved::Unresolved {
            name: r.name,
        },
        sites: vec![Site {
            file: path.to_string(),
            line,
            col,
        }],
        attributes: vec![template_attr_to_slug(r.attribute).to_string()],
    }
}

const fn template_attr_to_slug(attr: TemplateAttribute) -> &'static str {
    attr.as_str()
}

fn byte_offset_to_line_col(content: &str, offset: usize) -> (u32, u32) {
    let bytes = content.as_bytes();
    let end = offset.min(bytes.len());
    let mut line = 1u32;
    let mut col = 0u32;
    for &b in &bytes[..end] {
        if b == b'\n' {
            line += 1;
            col = 0;
        } else {
            col += 1;
        }
    }
    (line, col)
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
        assert_eq!(
            dispatch("src/routes/+page.svelte"),
            Some(Dispatch::Svelte)
        );
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

    #[test]
    fn byte_offset_to_line_col_first_line() {
        assert_eq!(byte_offset_to_line_col("hello\nworld", 0), (1, 0));
        assert_eq!(byte_offset_to_line_col("hello\nworld", 3), (1, 3));
    }

    #[test]
    fn byte_offset_to_line_col_after_newline() {
        assert_eq!(byte_offset_to_line_col("hello\nworld", 6), (2, 0));
        assert_eq!(byte_offset_to_line_col("hello\nworld", 8), (2, 2));
    }

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
}

use std::path::Path;

use standardoc_core::{ExtractContext, ExtractError, LanguageProvider};
use standardoc_ir::{
    BuiltinEntry, BuiltinTier, EdgeKind, ExtractedFile, Language, RawEdge, ResolvedOrUnresolved,
    Site,
};
use swc_core::ecma::parser::{EsSyntax, Syntax, TsSyntax};

use crate::builtins::global as global_builtin_registry;
use crate::c::CProvider;
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
    c: CProvider,
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
            Some(Dispatch::C) => self.c.extract(content, path, ctx),
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
    C,
    Vue,
    Svelte,
}

fn dispatch(path: &str) -> Option<Dispatch> {
    let ext = Path::new(path).extension()?.to_str()?;
    match ext {
        "rs" => Some(Dispatch::Rust),
        "ts" | "tsx" | "js" | "jsx" => Some(Dispatch::TypeScript),
        "lua" => Some(Dispatch::Lua),
        "c" | "h" => Some(Dispatch::C),
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
mod tests;

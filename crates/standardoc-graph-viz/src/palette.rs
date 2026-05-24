//! Theme palette, hydrated from CSS custom properties on the JS side.
//!
//! Defaults match VSCode's "Dark Modern" so the engine renders
//! something sensible before the host has a chance to push a real
//! palette. The host SHOULD call `set_palette(json)` once at boot.

use serde::{Deserialize, Serialize};

use crate::kind::Kind;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct Palette {
    #[serde(default = "default_background")]
    pub background: String,
    #[serde(default = "default_foreground")]
    pub foreground: String,
    #[serde(default = "default_description")]
    pub description: String,
    #[serde(default = "default_panel_border")]
    pub panel_border: String,
    #[serde(default = "default_focus_border")]
    pub focus_border: String,
    #[serde(default = "default_widget_background")]
    pub widget_background: String,
    #[serde(default = "default_list_hover")]
    pub list_hover: String,
    #[serde(default = "default_text_link")]
    pub text_link: String,
    #[serde(default = "default_edge_calls")]
    pub edge_calls: String,
    #[serde(default = "default_edge_imports")]
    pub edge_imports: String,
    #[serde(default = "default_edge_extends")]
    pub edge_extends: String,
    #[serde(default = "default_edge_implements")]
    pub edge_implements: String,
    #[serde(default = "default_edge_references")]
    pub edge_references: String,
    #[serde(default = "default_edge_uses_type")]
    pub edge_uses_type: String,
    #[serde(default = "default_lang_rust")]
    pub lang_rust: String,
    #[serde(default = "default_lang_typescript")]
    pub lang_typescript: String,
    #[serde(default = "default_lang_javascript")]
    pub lang_javascript: String,
    #[serde(default = "default_lang_lua")]
    pub lang_lua: String,
    #[serde(default = "default_lang_vue")]
    pub lang_vue: String,
    #[serde(default = "default_lang_svelte")]
    pub lang_svelte: String,
    #[serde(default = "default_lang_c")]
    pub lang_c: String,
    #[serde(default = "default_lang_default")]
    pub lang_default: String,
    #[serde(default = "default_proj_rust")]
    pub proj_rust: String,
    #[serde(default = "default_proj_node")]
    pub proj_node: String,
    #[serde(default = "default_proj_bun")]
    pub proj_bun: String,
    #[serde(default = "default_proj_deno")]
    pub proj_deno: String,
    #[serde(default = "default_proj_python")]
    pub proj_python: String,
    #[serde(default = "default_proj_lua")]
    pub proj_lua: String,
    #[serde(default = "default_proj_c")]
    pub proj_c: String,
    #[serde(default = "default_proj_cpp")]
    pub proj_cpp: String,
    #[serde(default = "default_proj_default")]
    pub proj_default: String,
}

impl Default for Palette {
    fn default() -> Self {
        Self {
            background: default_background(),
            foreground: default_foreground(),
            description: default_description(),
            panel_border: default_panel_border(),
            focus_border: default_focus_border(),
            widget_background: default_widget_background(),
            list_hover: default_list_hover(),
            text_link: default_text_link(),
            edge_calls: default_edge_calls(),
            edge_imports: default_edge_imports(),
            edge_extends: default_edge_extends(),
            edge_implements: default_edge_implements(),
            edge_references: default_edge_references(),
            edge_uses_type: default_edge_uses_type(),
            lang_rust: default_lang_rust(),
            lang_typescript: default_lang_typescript(),
            lang_javascript: default_lang_javascript(),
            lang_lua: default_lang_lua(),
            lang_vue: default_lang_vue(),
            lang_svelte: default_lang_svelte(),
            lang_c: default_lang_c(),
            lang_default: default_lang_default(),
            proj_rust: default_proj_rust(),
            proj_node: default_proj_node(),
            proj_bun: default_proj_bun(),
            proj_deno: default_proj_deno(),
            proj_python: default_proj_python(),
            proj_lua: default_proj_lua(),
            proj_c: default_proj_c(),
            proj_cpp: default_proj_cpp(),
            proj_default: default_proj_default(),
        }
    }
}

impl Palette {
    /// Color used by the renderer for a given `edge_kind` string. Falls
    /// back to `foreground` for unknown kinds so a future IR addition
    /// doesn't paint invisible.
    #[must_use]
    pub(crate) fn edge_color(&self, kind: &str) -> &str {
        match kind {
            "CALLS" => &self.edge_calls,
            "IMPORTS" => &self.edge_imports,
            "EXTENDS" => &self.edge_extends,
            "IMPLEMENTS" => &self.edge_implements,
            "REFERENCES" => &self.edge_references,
            "USES_TYPE" => &self.edge_uses_type,
            _ => &self.foreground,
        }
    }

    /// Color of a chip's left accent bar, keyed on the symbol's
    /// `language` string (the serde-lowercased `Language` enum:
    /// `rust` / `typescript` / `javascript` / `lua` / `vue` /
    /// `svelte` / `c`). Falls back to `lang_default` so a future IR
    /// language doesn't paint invisible.
    #[must_use]
    pub(crate) fn language_color(&self, language: &str) -> &str {
        match language {
            "rust" => &self.lang_rust,
            "typescript" => &self.lang_typescript,
            "javascript" => &self.lang_javascript,
            "lua" => &self.lang_lua,
            "vue" => &self.lang_vue,
            "svelte" => &self.lang_svelte,
            "c" => &self.lang_c,
            _ => &self.lang_default,
        }
    }

    /// Color of a project frame's header band, keyed on the project
    /// `kind` (ecosystem tag — `rust` / `node` / `bun` / …). Falls
    /// back to `proj_default` for `custom:<tag>` / `unknown` / any
    /// future ecosystem.
    #[must_use]
    pub(crate) fn project_color(&self, kind: &str) -> &str {
        match kind {
            "rust" => &self.proj_rust,
            "node" => &self.proj_node,
            "bun" => &self.proj_bun,
            "deno" => &self.proj_deno,
            "python" => &self.proj_python,
            "lua" => &self.proj_lua,
            "c" => &self.proj_c,
            "cpp" => &self.proj_cpp,
            _ => &self.proj_default,
        }
    }
}

/// Per-`Kind` header / sphere colour for symbol cards. Hardcoded
/// because the JSON `Palette` contract is host-driven and we don't
/// want to grow it for V2 — promote these to palette fields if the
/// scheme needs to be themable. Shared by the 2D card renderer and
/// the 3D upload path so the two views read with one identity.
pub(crate) fn kind_color_hex(kind: Kind) -> &'static str {
    match kind {
        Kind::Module => "#b180d7",   // purple — namespaces
        Kind::Type => "#cca700",     // yellow — declarations
        Kind::Callable => "#3794ff", // blue — behaviour
        Kind::Value => "#89d185",    // green — values / consts
        Kind::Macro => "#f48771",    // orange — meta
        Kind::Unknown => "#9d9d9d",  // grey — catch-all
    }
}

/// Halo colour for Phase 3 (Flow) entry-point highlight, keyed on
/// the snake_case `EntryPointKind` wire tag (`binary_main` /
/// `public_api` / `ffi_export`). Returns `None` for any unknown tag
/// so a future IR variant simply paints no halo until the renderer
/// learns about it. Hardcoded for the same reason as `kind_color_hex`
/// — the JSON `Palette` is host-driven and we don't want to grow it
/// just for V3. Shared by the 2D card renderer and the 3D upload path.
pub(crate) fn entry_point_halo_color(tag: &str) -> Option<&'static str> {
    match tag {
        "binary_main" => Some("#4a9eff"), // cornflower blue — the program's root
        "public_api" => Some("#f5b942"),  // amber — public surface
        "ffi_export" => Some("#ff8a3a"),  // orange — foreign boundary
        _ => None,
    }
}

fn default_background() -> String {
    "#1e1e1e".into()
}
fn default_foreground() -> String {
    "#cccccc".into()
}
fn default_description() -> String {
    "#9d9d9d".into()
}
fn default_panel_border() -> String {
    "#454545".into()
}
fn default_focus_border() -> String {
    "#007fd4".into()
}
fn default_widget_background() -> String {
    "#252526".into()
}
fn default_list_hover() -> String {
    "#2a2d2e".into()
}
fn default_text_link() -> String {
    "#3794ff".into()
}
fn default_edge_calls() -> String {
    "#3794ff".into()
}
fn default_edge_imports() -> String {
    "#b180d7".into()
}
fn default_edge_extends() -> String {
    "#d18616".into()
}
fn default_edge_implements() -> String {
    "#cca700".into()
}
fn default_edge_references() -> String {
    "#cccccc".into()
}
fn default_edge_uses_type() -> String {
    "#f48771".into()
}

// Language accent colors — GitHub linguist palette.
fn default_lang_rust() -> String {
    "#dea584".into()
}
fn default_lang_typescript() -> String {
    "#3178c6".into()
}
fn default_lang_javascript() -> String {
    "#f1e05a".into()
}
fn default_lang_lua() -> String {
    "#000080".into()
}
fn default_lang_vue() -> String {
    "#41b883".into()
}
fn default_lang_svelte() -> String {
    "#ff3e00".into()
}
fn default_lang_c() -> String {
    "#555555".into()
}
fn default_lang_default() -> String {
    "#8b949e".into()
}

// Project frame header-band colors — saturated enough to carry
// light header text, one distinct hue per ecosystem.
fn default_proj_rust() -> String {
    "#c56a1c".into()
}
fn default_proj_node() -> String {
    "#cb9b00".into()
}
fn default_proj_bun() -> String {
    "#9b8b6a".into()
}
fn default_proj_deno() -> String {
    "#4d4d4d".into()
}
fn default_proj_python() -> String {
    "#3572a5".into()
}
fn default_proj_lua() -> String {
    "#2d2d80".into()
}
fn default_proj_c() -> String {
    "#5c6370".into()
}
fn default_proj_cpp() -> String {
    "#9c4668".into()
}
fn default_proj_default() -> String {
    "#3a3a3a".into()
}

//! Theme palette, hydrated from CSS custom properties on the JS side.
//!
//! Defaults match VSCode's "Dark Modern" so the engine renders
//! something sensible before the host has a chance to push a real
//! palette. The host SHOULD call `set_palette(json)` once at boot.

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
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
    #[serde(default = "default_edge_defines")]
    pub edge_defines: String,
    #[serde(default = "default_edge_uses_type")]
    pub edge_uses_type: String,
    #[serde(default = "default_edge_exposes_api")]
    pub edge_exposes_api: String,
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
            edge_defines: default_edge_defines(),
            edge_uses_type: default_edge_uses_type(),
            edge_exposes_api: default_edge_exposes_api(),
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
            "DEFINES" => &self.edge_defines,
            "USES_TYPE" => &self.edge_uses_type,
            "EXPOSES_API" => &self.edge_exposes_api,
            _ => &self.foreground,
        }
    }
}

fn default_background() -> String { "#1e1e1e".into() }
fn default_foreground() -> String { "#cccccc".into() }
fn default_description() -> String { "#9d9d9d".into() }
fn default_panel_border() -> String { "#454545".into() }
fn default_focus_border() -> String { "#007fd4".into() }
fn default_widget_background() -> String { "#252526".into() }
fn default_list_hover() -> String { "#2a2d2e".into() }
fn default_text_link() -> String { "#3794ff".into() }
fn default_edge_calls() -> String { "#3794ff".into() }
fn default_edge_imports() -> String { "#b180d7".into() }
fn default_edge_extends() -> String { "#d18616".into() }
fn default_edge_implements() -> String { "#cca700".into() }
fn default_edge_references() -> String { "#cccccc".into() }
fn default_edge_defines() -> String { "#89d185".into() }
fn default_edge_uses_type() -> String { "#f48771".into() }
fn default_edge_exposes_api() -> String { "#b180d7".into() }

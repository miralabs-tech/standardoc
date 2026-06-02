//! Per-`Kind` symbol colour for card rendering.
//!
//! The host-driven JSON `Palette` (CSS custom properties pushed via a
//! `set_palette` entry point) was dropped with the legacy `GraphEngine`
//! in Phase 3d. The slim canvases hardcode their edge / language /
//! project colours inline (`overview::cross_edge_style`,
//! `focus::edge_color_for_kind`); only the per-`Kind` card colour
//! survives here, shared by the focus card renderer.

use crate::kind::Kind;

/// Per-`Kind` header / sphere colour for symbol cards. Hardcoded
/// because the colour scheme is not host-themable today; promote to a
/// host-driven palette if that changes. Shared by the 2D card renderer
/// and the 3D upload path so the two views read with one identity.
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

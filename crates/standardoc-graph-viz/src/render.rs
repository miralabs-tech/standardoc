//! Canvas2D renderer for the **current drill level** — cards + edges.
//!
//! One entrypoint, [`draw`], called from `GraphEngine::tick` once per
//! frame. The renderer is stateless: it reads scene + viewport +
//! interaction state and paints, no caching. The previous LOD tiers
//! (project / module / chip) are gone: deconstruction is now driven
//! by drill navigation, not by zoom level. The scene only ever
//! contains the focused node's direct children, so there's nothing
//! left to fade in/out per zoom.

#![allow(clippy::cast_possible_truncation, clippy::cast_precision_loss, clippy::cast_sign_loss)]

use wasm_bindgen::JsValue;
use web_sys::CanvasRenderingContext2d;

use crate::interaction::InteractionState;
use crate::palette::{Palette, entry_point_halo_color, kind_color_hex};
use crate::scene::{Bounds, Card, Scene};
use crate::viewport::Viewport;

const CARD_RADIUS: f64 = 10.0;
const HEADER_H: f64 = 32.0;

const MINIMAP_W: f64 = 180.0;
const MINIMAP_H: f64 = 130.0;
const MINIMAP_MARGIN: f64 = 16.0;
const MINIMAP_PAD: f64 = 8.0;

pub(crate) fn draw(
    ctx: &CanvasRenderingContext2d,
    width: f64,
    height: f64,
    dpr: f64,
    scene: &Scene,
    viewport: &Viewport,
    interaction: &InteractionState,
    palette: &Palette,
) {
    // Background fill in identity transform so we paint the whole
    // backbuffer regardless of the world transform.
    let _ = ctx.set_transform(1.0, 0.0, 0.0, 1.0, 0.0, 0.0);
    ctx.set_fill_style_str(&palette.background);
    ctx.fill_rect(0.0, 0.0, width * dpr, height * dpr);

    if scene.cards.is_empty() {
        draw_empty_message(ctx, width, height, dpr, palette);
        return;
    }

    // World transform — HiDPI × viewport. Subsequent ops are in
    // world units; pixel-thin strokes need 1/scale compensation to
    // stay visible.
    let s = dpr * viewport.scale;
    let tx = dpr * viewport.offset_x;
    let ty = dpr * viewport.offset_y;
    let _ = ctx.set_transform(s, 0.0, 0.0, s, tx, ty);

    let hovered_fqdn = interaction.hovered_ref();
    let hovered_card = hovered_fqdn.and_then(|f| scene.card_by_fqdn.get(f).copied());

    draw_edges(ctx, scene, palette, viewport.scale, hovered_card);
    draw_cards(ctx, scene, palette, viewport.scale, hovered_card);

    // Minimap overlay — paint in screen space, on top of everything.
    let _ = ctx.set_transform(dpr, 0.0, 0.0, dpr, 0.0, 0.0);
    draw_minimap(ctx, width, height, scene, viewport, palette);
}

fn draw_edges(
    ctx: &CanvasRenderingContext2d,
    scene: &Scene,
    palette: &Palette,
    scale: f64,
    hovered: Option<usize>,
) {
    if scene.edges.is_empty() {
        return;
    }
    // Edges at the current level are cross-subtree aggregations
    // (when `kind` is empty) OR hover-specific kind-tagged edges
    // (when `kind` is set). Aggregated edges use `text_link` so they
    // read clearly against the dark canvas background — the previous
    // `panel_border` choice was too close to the bg to convey
    // dependency density.
    let base_w = (2.0_f64 / scale).max(0.4);
    let highlight_w = (3.5_f64 / scale).max(0.6);
    let dash_ghost = js_sys::Array::of2(&JsValue::from_f64(10.0), &JsValue::from_f64(6.0));
    let dash_solid = js_sys::Array::new();
    for e in &scene.edges {
        let from = &scene.cards[e.from_card];
        let to = &scene.cards[e.to_card];
        let fx = from.x + from.w * 0.5;
        let fy = from.y + from.h * 0.5;
        let tx_ = to.x + to.w * 0.5;
        let ty_ = to.y + to.h * 0.5;
        let involved = matches!(hovered, Some(h) if h == e.from_card || h == e.to_card);
        let touches_ghost = from.is_ghost || to.is_ghost;
        let color: &str = if e.kind.is_empty() {
            &palette.text_link
        } else {
            palette.edge_color(&e.kind)
        };
        ctx.set_stroke_style_str(color);
        // Phase 3 (Flow) 3.4 — edge weight modulates thickness so the
        // dependency density between two subtrees reads at a glance.
        // `weight == 1` keeps the existing base_w; each extra link
        // grows the line by ~25%, capped at 3× so a 50-link edge isn't
        // overwhelming. Hover override still uses highlight_w.
        let weight_factor = (1.0 + f64::from(e.weight.saturating_sub(1)) * 0.25).min(3.0);
        let edge_w = if involved {
            highlight_w
        } else {
            base_w * weight_factor
        };
        ctx.set_line_width(edge_w);
        let base_alpha = if touches_ghost { 0.45 } else { 0.75 };
        ctx.set_global_alpha(if hovered.is_some() && !involved { 0.25 } else { base_alpha });
        let _ = ctx.set_line_dash(if touches_ghost { dash_ghost.as_ref() } else { dash_solid.as_ref() });
        ctx.begin_path();
        ctx.move_to(fx, fy);
        ctx.line_to(tx_, ty_);
        ctx.stroke();
    }
    let _ = ctx.set_line_dash(dash_solid.as_ref());
    ctx.set_global_alpha(1.0);
}

fn draw_cards(
    ctx: &CanvasRenderingContext2d,
    scene: &Scene,
    palette: &Palette,
    scale: f64,
    hovered: Option<usize>,
) {
    let stroke_thin = (1.0_f64 / scale).max(0.25);
    let stroke_hover = (2.5_f64 / scale).max(0.4);
    let dash_ghost = js_sys::Array::of2(&JsValue::from_f64(8.0), &JsValue::from_f64(6.0));
    let dash_solid = js_sys::Array::new();
    for (i, c) in scene.cards.iter().enumerate() {
        let is_hovered = hovered == Some(i);
        // Ghost cards represent sibling-of-focus containers the
        // focused subtree couples to — dim the whole card via
        // globalAlpha so it reads as "context, not current level",
        // and switch to a dashed border for the same affordance.
        let card_alpha = if c.is_ghost { 0.45 } else { 1.0 };
        ctx.set_global_alpha(card_alpha);
        // Phase 3 (Flow) halo — two concentric rounded fills behind
        // the card body, mimicking the soft falloff of the 3D shader
        // halo (outer ring fainter, inner ring closer to the body
        // edge brighter). Skipped when the symbol is not an entry
        // point or the tag is unknown to the palette.
        if let Some(halo_hex) = c.entry_point.as_deref().and_then(entry_point_halo_color) {
            ctx.set_fill_style_str(halo_hex);
            ctx.set_global_alpha(card_alpha * 0.18);
            fill_round_rect(ctx, c.x - 12.0, c.y - 12.0, c.w + 24.0, c.h + 24.0, CARD_RADIUS + 12.0);
            ctx.set_global_alpha(card_alpha * 0.35);
            fill_round_rect(ctx, c.x - 6.0, c.y - 6.0, c.w + 12.0, c.h + 12.0, CARD_RADIUS + 6.0);
            ctx.set_global_alpha(card_alpha);
        }
        // Body
        ctx.set_fill_style_str(if is_hovered {
            &palette.list_hover
        } else {
            &palette.widget_background
        });
        fill_round_rect(ctx, c.x, c.y, c.w, c.h, CARD_RADIUS);
        // Header band — language or project color
        let header_color = card_header_color(c, palette);
        ctx.set_fill_style_str(header_color);
        fill_round_rect_top(ctx, c.x, c.y, c.w, HEADER_H.min(c.h), CARD_RADIUS);
        // Border (dashed for ghosts)
        ctx.set_stroke_style_str(if is_hovered {
            &palette.focus_border
        } else {
            &palette.panel_border
        });
        ctx.set_line_width(if is_hovered { stroke_hover } else { stroke_thin });
        let _ = ctx.set_line_dash(if c.is_ghost { dash_ghost.as_ref() } else { dash_solid.as_ref() });
        stroke_round_rect(ctx, c.x, c.y, c.w, c.h, CARD_RADIUS);
        // Label on the header band
        let label = if c.display_label.is_empty() {
            c.label.as_str()
        } else {
            c.display_label.as_str()
        };
        ctx.set_font("600 14px system-ui, sans-serif");
        ctx.set_text_baseline("middle");
        ctx.set_text_align("start");
        ctx.set_fill_style_str(&palette.foreground);
        let _ = ctx.fill_text(label, c.x + 12.0, c.y + HEADER_H * 0.5);
        // Sub-info (footer): descendant count for containers, fqdn
        // hint for leaves.
        let sub = card_subtitle(c);
        ctx.set_font("400 11px system-ui, sans-serif");
        ctx.set_fill_style_str(&palette.description);
        ctx.set_text_baseline("alphabetic");
        let _ = ctx.fill_text(&sub, c.x + 12.0, c.y + c.h - 12.0);
    }
    ctx.set_global_alpha(1.0);
    let _ = ctx.set_line_dash(dash_solid.as_ref());
}

fn card_subtitle(c: &Card) -> String {
    if c.is_container {
        match c.descendant_count {
            0 => "empty".to_string(),
            1 => "1 descendant".to_string(),
            n => format!("{n} descendants"),
        }
    } else if c.fqdn.is_empty() {
        String::new()
    } else {
        // Strip the project segment from a leaf fqdn so the footer
        // shows a relative path (the project label is already up in
        // the breadcrumb).
        match c.fqdn.split_once("::") {
            Some((_, rest)) => rest.to_string(),
            None => c.fqdn.clone(),
        }
    }
}

fn card_header_color<'a>(card: &Card, palette: &'a Palette) -> &'a str {
    // Empty fqdn ⇒ synthetic project node, coloured by ecosystem
    // kind (`rust` / `node` / `bun` / …) — that's the more
    // identifying signal at the workspace overview.
    //
    // Inside a project, the language is almost always uniform
    // (all `rust` in a Cargo crate, all `typescript` in a bun
    // workspace) so colouring by language would flatten the level
    // to one hue. Symbol *kind* — Module / Type / Function / … —
    // carries the actual variation users navigate against, so leaf
    // cards take their header colour from `kind_color_hex` (shared
    // with the 3D upload so both views read with one identity).
    if card.fqdn.is_empty() {
        palette.project_color(&card.language)
    } else {
        kind_color_hex(card.kind)
    }
}

// ---- Canvas2D helpers (also used by `Scene::prepare_labels`) -------

/// Truncate `text` to fit in `max_w` pixels at the current ctx font,
/// appending `…` when truncation happens. O(log n) on text length via
/// binary search over prefix widths. No-op when the text already fits.
pub(crate) fn truncate_to_width(
    ctx: &CanvasRenderingContext2d,
    text: &str,
    max_w: f64,
) -> String {
    if text.is_empty() || max_w <= 0.0 {
        return text.to_string();
    }
    let full = ctx.measure_text(text).map(|m| m.width()).unwrap_or(0.0);
    if full <= max_w {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return String::new();
    }
    let ellipsis = "…";
    let mut lo = 0usize;
    let mut hi = chars.len();
    while lo + 1 < hi {
        let mid = (lo + hi) / 2;
        let candidate: String = chars[..mid].iter().collect::<String>() + ellipsis;
        let w = ctx
            .measure_text(&candidate)
            .map(|m| m.width())
            .unwrap_or(0.0);
        if w <= max_w {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let mut out: String = chars[..lo].iter().collect();
    out.push_str(ellipsis);
    out
}

fn fill_round_rect(
    ctx: &CanvasRenderingContext2d,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    r: f64,
) {
    round_rect_path(ctx, x, y, w, h, r);
    ctx.fill();
}

fn stroke_round_rect(
    ctx: &CanvasRenderingContext2d,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    r: f64,
) {
    round_rect_path(ctx, x, y, w, h, r);
    ctx.stroke();
}

fn round_rect_path(
    ctx: &CanvasRenderingContext2d,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    r: f64,
) {
    let r = r.min(w * 0.5).min(h * 0.5).max(0.0);
    ctx.begin_path();
    ctx.move_to(x + r, y);
    ctx.line_to(x + w - r, y);
    let _ = ctx.arc_to(x + w, y, x + w, y + r, r);
    ctx.line_to(x + w, y + h - r);
    let _ = ctx.arc_to(x + w, y + h, x + w - r, y + h, r);
    ctx.line_to(x + r, y + h);
    let _ = ctx.arc_to(x, y + h, x, y + h - r, r);
    ctx.line_to(x, y + r);
    let _ = ctx.arc_to(x, y, x + r, y, r);
    ctx.close_path();
}

/// Round-rect with rounded TOP corners only — for the header band
/// sitting flush with the card top edge.
fn fill_round_rect_top(
    ctx: &CanvasRenderingContext2d,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    r: f64,
) {
    let r = r.min(w * 0.5).min(h).max(0.0);
    ctx.begin_path();
    ctx.move_to(x + r, y);
    ctx.line_to(x + w - r, y);
    let _ = ctx.arc_to(x + w, y, x + w, y + r, r);
    ctx.line_to(x + w, y + h);
    ctx.line_to(x, y + h);
    ctx.line_to(x, y + r);
    let _ = ctx.arc_to(x, y, x + r, y, r);
    ctx.close_path();
    ctx.fill();
}

// ---- Minimap --------------------------------------------------------

/// Screen-space rect of the minimap overlay (`x`, `y`, `w`, `h`).
fn minimap_screen_rect(width: f64, height: f64) -> (f64, f64, f64, f64) {
    let x = width - MINIMAP_W - MINIMAP_MARGIN;
    let y = height - MINIMAP_H - MINIMAP_MARGIN;
    (x, y, MINIMAP_W, MINIMAP_H)
}

/// Translate a screen-space click inside the minimap rect to its
/// corresponding world coordinate. Returns `None` when the click
/// missed the minimap. Used by `GraphEngine::on_pointer_*` to
/// teleport the viewport on minimap clicks.
pub(crate) fn minimap_world_target(
    width: f64,
    height: f64,
    bounds: Bounds,
    sx: f64,
    sy: f64,
) -> Option<(f64, f64)> {
    if !bounds.is_valid() {
        return None;
    }
    let (mx, my, mw, mh) = minimap_screen_rect(width, height);
    if sx < mx || sx > mx + mw || sy < my || sy > my + mh {
        return None;
    }
    let avail_w = mw - 2.0 * MINIMAP_PAD;
    let avail_h = mh - 2.0 * MINIMAP_PAD;
    let bw = bounds.width().max(1.0);
    let bh = bounds.height().max(1.0);
    let scale = (avail_w / bw).min(avail_h / bh);
    let draw_w = bw * scale;
    let draw_h = bh * scale;
    let origin_x = mx + (mw - draw_w) * 0.5;
    let origin_y = my + (mh - draw_h) * 0.5;
    let local_x = (sx - origin_x).clamp(0.0, draw_w);
    let local_y = (sy - origin_y).clamp(0.0, draw_h);
    Some((
        bounds.min_x + local_x / scale,
        bounds.min_y + local_y / scale,
    ))
}

fn draw_minimap(
    ctx: &CanvasRenderingContext2d,
    width: f64,
    height: f64,
    scene: &Scene,
    viewport: &Viewport,
    palette: &Palette,
) {
    let bounds = scene.bounds();
    if !bounds.is_valid() || scene.cards.is_empty() {
        return;
    }
    let (mx, my, mw, mh) = minimap_screen_rect(width, height);
    // Panel
    ctx.set_fill_style_str(&palette.widget_background);
    fill_round_rect(ctx, mx, my, mw, mh, 6.0);
    ctx.set_stroke_style_str(&palette.panel_border);
    ctx.set_line_width(1.0);
    stroke_round_rect(ctx, mx, my, mw, mh, 6.0);

    let avail_w = mw - 2.0 * MINIMAP_PAD;
    let avail_h = mh - 2.0 * MINIMAP_PAD;
    let bw = bounds.width().max(1.0);
    let bh = bounds.height().max(1.0);
    let scale = (avail_w / bw).min(avail_h / bh);
    let draw_w = bw * scale;
    let draw_h = bh * scale;
    let origin_x = mx + (mw - draw_w) * 0.5;
    let origin_y = my + (mh - draw_h) * 0.5;
    let to_mm_x = |wx: f64| origin_x + (wx - bounds.min_x) * scale;
    let to_mm_y = |wy: f64| origin_y + (wy - bounds.min_y) * scale;

    ctx.set_fill_style_str(&palette.panel_border);
    for c in &scene.cards {
        let x = to_mm_x(c.x);
        let y = to_mm_y(c.y);
        let w = (c.w * scale).max(1.0);
        let h = (c.h * scale).max(1.0);
        ctx.fill_rect(x, y, w, h);
    }

    // Viewport box inside the minimap.
    let view_x0 = -viewport.offset_x / viewport.scale;
    let view_y0 = -viewport.offset_y / viewport.scale;
    let view_x1 = view_x0 + width / viewport.scale;
    let view_y1 = view_y0 + height / viewport.scale;
    let vx0 = to_mm_x(view_x0);
    let vy0 = to_mm_y(view_y0);
    let vx1 = to_mm_x(view_x1);
    let vy1 = to_mm_y(view_y1);
    ctx.set_stroke_style_str(&palette.focus_border);
    ctx.set_line_width(1.5);
    ctx.stroke_rect(vx0, vy0, (vx1 - vx0).max(2.0), (vy1 - vy0).max(2.0));
}

fn draw_empty_message(
    ctx: &CanvasRenderingContext2d,
    width: f64,
    height: f64,
    dpr: f64,
    palette: &Palette,
) {
    let _ = ctx.set_transform(dpr, 0.0, 0.0, dpr, 0.0, 0.0);
    ctx.set_fill_style_str(&palette.description);
    ctx.set_font("italic 14px system-ui, sans-serif");
    ctx.set_text_align("center");
    ctx.set_text_baseline("middle");
    let _ = ctx.fill_text("nothing to draw at this level", width * 0.5, height * 0.5);
    ctx.set_text_align("start");
}

//! Canvas 2D rendering. One entrypoint, [`draw`], called from
//! `GraphEngine::tick` once per frame. The renderer is stateless: it
//! reads scene + viewport + interaction state and paints — no
//! retained side effects between frames.

use web_sys::CanvasRenderingContext2d;

use crate::interaction::InteractionState;
use crate::palette::Palette;
use crate::scene::{Node, Scene};
use crate::viewport::Viewport;

const CLUSTER_RADIUS: f64 = 6.0;
const CHIP_RADIUS: f64 = 4.0;
const FONT_PX: f64 = 12.0;
const HEADER_FONT_PX: f64 = 12.0;

// LOD thresholds — scale ratios below which we skip increasingly
// expensive draw work. Without these, a "fit-everything" zoom on a
// 1 k+ symbol graph would draw 3 + N text labels per frame at sizes
// well below the eye's reading threshold, paying for invisible
// pixels. Numbers are scale = viewport.scale (1.0 = unzoomed).
const LOD_TEXT_MIN_SCALE: f64 = 0.5; // chip name + cluster title
const LOD_GLYPH_MIN_SCALE: f64 = 0.4; // kind glyph (fn / T / val …)
const LOD_OUTLINE_MIN_SCALE: f64 = 0.2; // chip stroke (rect outlines)

/// World-space axis-aligned bounding box of the current viewport.
/// Anything outside it is culled before we walk the chip list.
#[derive(Debug, Clone, Copy)]
struct ViewportBox {
    min_x: f64,
    min_y: f64,
    max_x: f64,
    max_y: f64,
}

impl ViewportBox {
    fn from_screen(viewport: &Viewport, width: f64, height: f64) -> Self {
        let (min_x, min_y) = viewport.screen_to_world(0.0, 0.0);
        let (max_x, max_y) = viewport.screen_to_world(width, height);
        // Margin in world units so chips that are partly off-screen
        // still paint their visible edge cleanly.
        let margin = 32.0;
        Self {
            min_x: min_x - margin,
            min_y: min_y - margin,
            max_x: max_x + margin,
            max_y: max_y + margin,
        }
    }

    fn intersects_rect(self, x: f64, y: f64, w: f64, h: f64) -> bool {
        x + w >= self.min_x && x <= self.max_x && y + h >= self.min_y && y <= self.max_y
    }
}

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
    // Clear in identity transform so we paint the whole backbuffer.
    let _ = ctx.set_transform(1.0, 0.0, 0.0, 1.0, 0.0, 0.0);
    ctx.set_fill_style_str(&palette.background);
    ctx.fill_rect(0.0, 0.0, width * dpr, height * dpr);

    if scene.nodes.is_empty() {
        draw_empty_message(ctx, width, height, dpr, palette);
        return;
    }

    // World transform: HiDPI * viewport * world. Subsequent drawing
    // operates in world units; pixel-thin strokes need to scale with
    // viewport scale to stay visible, see `stroke_world_px`.
    let s = dpr * viewport.scale;
    let tx = dpr * viewport.offset_x;
    let ty = dpr * viewport.offset_y;
    let _ = ctx.set_transform(s, 0.0, 0.0, s, tx, ty);

    let hovered_fqdn = interaction.hovered_ref();
    let view_box = ViewportBox::from_screen(viewport, width, height);

    draw_clusters(ctx, scene, palette, viewport.scale, view_box);
    draw_sections(ctx, scene, palette, viewport.scale, view_box);
    draw_nodes(ctx, scene, palette, viewport.scale, hovered_fqdn, view_box);
    if hovered_fqdn.is_some() {
        draw_edges_for_hovered(ctx, scene, palette, viewport.scale, hovered_fqdn);
    }
}

fn draw_clusters(
    ctx: &CanvasRenderingContext2d,
    scene: &Scene,
    palette: &Palette,
    scale: f64,
    view_box: ViewportBox,
) {
    // Two-pass body fill: intermediates first (stroke only, so they
    // frame the nesting without occluding their children) then leaves
    // (filled). Intermediate-with-own-symbols nodes get both — the
    // frame AND the chip-area fill — because they behave like leaves
    // for the chips' background and like intermediates for the
    // sub-containers nested inside.
    ctx.set_stroke_style_str(&palette.panel_border);
    ctx.set_line_width(stroke_world_px(1.0, scale));
    for n in &scene.hierarchy.nodes {
        if n.children.is_empty() {
            continue;
        }
        let (x, y, w, h) = (n.x as f64, n.y as f64, n.w as f64, n.h as f64);
        if !view_box.intersects_rect(x, y, w, h) {
            continue;
        }
        stroke_round_rect(ctx, x, y, w, h, CLUSTER_RADIUS);
    }

    ctx.set_fill_style_str(&palette.widget_background);
    for n in &scene.hierarchy.nodes {
        if !n.children.is_empty() {
            continue;
        }
        let (x, y, w, h) = (n.x as f64, n.y as f64, n.w as f64, n.h as f64);
        if !view_box.intersects_rect(x, y, w, h) {
            continue;
        }
        fill_round_rect(ctx, x, y, w, h, CLUSTER_RADIUS);
        stroke_round_rect(ctx, x, y, w, h, CLUSTER_RADIUS);
    }

    // Below the readability threshold we skip every header text and
    // count badge — they're sub-pixel-blur smears at full-overview
    // zoom and dominate the frame cost on dense graphs.
    if scale < LOD_TEXT_MIN_SCALE {
        return;
    }

    let header_font = format!("600 {HEADER_FONT_PX}px system-ui, sans-serif");
    let subtitle_font = "10px system-ui, sans-serif";
    let count_font = "10px system-ui, sans-serif";
    ctx.set_text_baseline("middle");
    for n in &scene.hierarchy.nodes {
        let (x, y, w, h) = (n.x as f64, n.y as f64, n.w as f64, n.h as f64);
        if !view_box.intersects_rect(x, y, w, h) {
            continue;
        }
        // Title (segment) — top line.
        ctx.set_font(&header_font);
        ctx.set_fill_style_str(&palette.text_link);
        ctx.set_text_align("left");
        let title = if n.display_title.is_empty() {
            n.segment.as_str()
        } else {
            n.display_title.as_str()
        };
        let _ = ctx.fill_text(title, x + 12.0, y + 14.0);

        // Count, in muted style, drawn just after the title — keeps
        // it adjacent to the label instead of flying to the right
        // edge of a 2000+ px container. Own-symbol count for leaves,
        // recursive aggregate for intermediates.
        let count = if n.children.is_empty() {
            n.symbol_indices.len() as u32
        } else {
            n.recursive_symbol_count
        };
        // Approximate title width — measure_text per-frame per-node
        // costs ~1 ms on dense graphs (see `truncate_to_width` for
        // history). `600 12px system-ui` is ~7 px per char on
        // average; the count placement is forgiving on exact pixels.
        let title_width = title.chars().count() as f64 * 7.0;
        ctx.set_font(count_font);
        ctx.set_fill_style_str(&palette.description);
        let _ = ctx.fill_text(&format!("({count})"), x + 12.0 + title_width + 6.0, y + 14.0);

        // Subtitle (full module path) — second line, description
        // style. Skipped when subtitle is empty (root nodes whose
        // segment IS the full path; no point in repeating it).
        if !n.display_subtitle.is_empty() {
            ctx.set_font(subtitle_font);
            ctx.set_fill_style_str(&palette.description);
            ctx.set_text_align("left");
            let _ = ctx.fill_text(&n.display_subtitle, x + 12.0, y + 30.0);
        }
    }
}

/// Paint per-Kind section headers inside every owner that has its
/// own chips. Each header carries a triangle affordance (`▾` for
/// expanded, `▸` for collapsed), the kind label, and the chip count
/// in muted style. The toggle interactivity comes in Phase B.2 —
/// for now the triangle is purely a visual cue about the load-time
/// default-collapse heuristic (sections with > threshold start
/// collapsed).
fn draw_sections(
    ctx: &CanvasRenderingContext2d,
    scene: &Scene,
    palette: &Palette,
    scale: f64,
    view_box: ViewportBox,
) {
    // Sections are sub-pixel-blur at low zoom — same LOD policy as
    // headers above.
    if scale < LOD_TEXT_MIN_SCALE {
        return;
    }
    let section_font = "600 11px system-ui, sans-serif";
    let count_font = "10px system-ui, sans-serif";
    ctx.set_text_baseline("middle");
    for n in &scene.hierarchy.nodes {
        if n.sections.is_empty() {
            continue;
        }
        let (x, y, w, h) = (n.x as f64, n.y as f64, n.w as f64, n.h as f64);
        if !view_box.intersects_rect(x, y, w, h) {
            continue;
        }
        let chips_origin_x = x + crate::layout::CONTAINER_PADDING as f64;
        let chips_origin_y = y + crate::layout::CONTAINER_HEADER_H as f64;
        let strip_inner_h = crate::layout::SECTION_HEADER_H as f64;
        for section in &n.sections {
            let strip_y = chips_origin_y + section.y_offset as f64;
            // Triangle (left): ▾ when expanded, ▸ when collapsed.
            let glyph = if section.expanded { "▾" } else { "▸" };
            ctx.set_font(section_font);
            ctx.set_fill_style_str(&palette.text_link);
            ctx.set_text_align("left");
            let cy = strip_y + strip_inner_h * 0.5;
            let _ = ctx.fill_text(glyph, chips_origin_x, cy);
            // Kind label, right of the triangle.
            let label = section.kind.section_label();
            let _ = ctx.fill_text(label, chips_origin_x + 16.0, cy);
            // Count in parens, just after the label — keeps it
            // adjacent instead of flying to the far right of a
            // wide container. Approximate label width per
            // `600 11px system-ui` (~6.5 px/char).
            let label_width = label.chars().count() as f64 * 6.5;
            ctx.set_font(count_font);
            ctx.set_fill_style_str(&palette.description);
            let _ = ctx.fill_text(
                &format!("({})", section.chip_count),
                chips_origin_x + 16.0 + label_width + 5.0,
                cy,
            );
        }
    }
}

fn draw_nodes(
    ctx: &CanvasRenderingContext2d,
    scene: &Scene,
    palette: &Palette,
    scale: f64,
    hovered_fqdn: Option<&str>,
    view_box: ViewportBox,
) {
    let draw_text = scale >= LOD_TEXT_MIN_SCALE;
    let draw_glyph = scale >= LOD_GLYPH_MIN_SCALE;
    let draw_outline = scale >= LOD_OUTLINE_MIN_SCALE;

    if draw_text {
        let chip_font = format!("{FONT_PX}px system-ui, sans-serif");
        ctx.set_font(&chip_font);
        ctx.set_text_baseline("middle");
    }
    for n in &scene.nodes {
        if !view_box.intersects_rect(n.x, n.y, n.w, n.h) {
            continue;
        }
        draw_node(
            ctx,
            n,
            palette,
            scale,
            hovered_fqdn,
            draw_text,
            draw_glyph,
            draw_outline,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_node(
    ctx: &CanvasRenderingContext2d,
    n: &Node,
    palette: &Palette,
    scale: f64,
    hovered_fqdn: Option<&str>,
    draw_text: bool,
    draw_glyph: bool,
    draw_outline: bool,
) {
    let hovered = hovered_fqdn == Some(n.fqdn.as_str());
    let fill = if hovered {
        &palette.list_hover
    } else {
        &palette.background
    };
    ctx.set_fill_style_str(fill);
    fill_round_rect(ctx, n.x, n.y, n.w, n.h, CHIP_RADIUS);

    // Below the outline LOD threshold the per-chip stroke costs more
    // than it adds visually (anti-aliased hairlines smear together).
    // Always draw the outline for the hovered chip so the focus
    // affordance survives even at zoomed-out overview.
    if draw_outline || hovered {
        let stroke = if hovered {
            &palette.focus_border
        } else {
            &palette.panel_border
        };
        ctx.set_stroke_style_str(stroke);
        ctx.set_line_width(stroke_world_px(if hovered { 1.8 } else { 1.0 }, scale));
        if n.is_external {
            ctx.set_global_alpha(0.65);
            ctx.set_line_dash(&js_sys::Array::of2(&3.0.into(), &2.0.into())).ok();
        }
        stroke_round_rect(ctx, n.x, n.y, n.w, n.h, CHIP_RADIUS);
        if n.is_external {
            ctx.set_global_alpha(1.0);
            ctx.set_line_dash(&js_sys::Array::new()).ok();
        }
    }

    if draw_text {
        ctx.set_fill_style_str(&palette.foreground);
        ctx.set_text_align("left");
        // Use precomputed truncated label — avoids a binary-search
        // measure_text loop per chip per frame. Falls back to the raw
        // name if `Scene::prepare_labels` hasn't run yet (e.g. the host
        // forgot to call it; the engine always does in `load_graph`).
        let label = if n.display_name.is_empty() {
            n.name.as_str()
        } else {
            n.display_name.as_str()
        };
        let _ = ctx.fill_text(label, n.x + 10.0, n.y + n.h * 0.5);
    }
    if draw_glyph {
        ctx.set_fill_style_str(&palette.description);
        ctx.set_text_align("right");
        // Prefer the language-specific tag (`method` / `impl_fn` /
        // `struct` / `trait` / `macro_rules` / ...) over the generic
        // `Kind` glyph when the payload carries it — it conveys the
        // precise role of the symbol. Falls back to the broad
        // `Kind::glyph` when the server omitted `language_kind`.
        let glyph: &str = if n.language_kind.is_empty() {
            n.kind.glyph()
        } else {
            n.language_kind.as_str()
        };
        let _ = ctx.fill_text(glyph, n.x + n.w - 10.0, n.y + n.h * 0.5);
    }
}

fn draw_edges_for_hovered(
    ctx: &CanvasRenderingContext2d,
    scene: &Scene,
    palette: &Palette,
    scale: f64,
    hovered_fqdn: Option<&str>,
) {
    let Some(fqdn) = hovered_fqdn else { return };
    let Some(&from_idx) = scene.node_by_fqdn.get(fqdn) else { return };
    ctx.set_line_width(stroke_world_px(2.0, scale));
    ctx.set_global_alpha(0.85);
    for e in &scene.edges {
        let touches = e.from_node == from_idx || e.to_node == from_idx;
        if !touches {
            continue;
        }
        let Some(from) = scene.nodes.get(e.from_node) else { continue };
        let Some(to) = scene.nodes.get(e.to_node) else { continue };
        let (fx, fy) = (from.x + from.w * 0.5, from.y + from.h * 0.5);
        let (tx, ty) = (to.x + to.w * 0.5, to.y + to.h * 0.5);
        let dx = tx - fx;
        let lift = (dx.abs() * 0.2).min(80.0);
        let cx = (fx + tx) * 0.5;
        let cy = (fy + ty) * 0.5 - if dx >= 0.0 { lift } else { -lift };
        // Arrowhead direction = tangent to the quadratic Bézier at its
        // end point. For Q(t) = (1-t)²·P0 + 2(1-t)t·P1 + t²·P2 the
        // derivative at t=1 is 2·(P2 − P1) — collinear with (P2 − P1),
        // which is all we need for a unit-length direction vector.
        let dir_x = tx - cx;
        let dir_y = ty - cy;
        let dir_len = dir_x.hypot(dir_y);
        if dir_len < 0.1 {
            // Degenerate edge (chip overlapping itself or zero-length):
            // draw the curve straight, no arrowhead. Don't crash on the
            // division below.
            ctx.set_stroke_style_str(palette.edge_color(&e.kind));
            ctx.begin_path();
            ctx.move_to(fx, fy);
            ctx.line_to(tx, ty);
            ctx.stroke();
            continue;
        }
        let ux = dir_x / dir_len;
        let uy = dir_y / dir_len;
        // Pull the arrow tip back from the target chip's centre so it
        // lands near the chip's incoming edge instead of getting
        // swallowed by the chip rectangle's fill. Half the smaller
        // chip dimension is a cheap approximation of "edge of the
        // chip" without per-rect ray-vs-AABB math.
        let pull_back = to.w.min(to.h) * 0.5;
        let tip_x = tx - ux * pull_back;
        let tip_y = ty - uy * pull_back;
        // Arrowhead size in screen pixels — divided by scale so the
        // head keeps the same visual size at all zoom levels.
        let arrow_len = 10.0 / scale.max(0.0001);
        let arrow_half_w = arrow_len * 0.45;
        let perp_x = -uy;
        let perp_y = ux;
        let base_x = tip_x - ux * arrow_len;
        let base_y = tip_y - uy * arrow_len;
        let left_x = base_x + perp_x * arrow_half_w;
        let left_y = base_y + perp_y * arrow_half_w;
        let right_x = base_x - perp_x * arrow_half_w;
        let right_y = base_y - perp_y * arrow_half_w;

        if e.kind == "REFERENCES" {
            ctx.set_line_dash(&js_sys::Array::of2(&4.0.into(), &3.0.into())).ok();
        } else {
            ctx.set_line_dash(&js_sys::Array::new()).ok();
        }
        let edge_color = palette.edge_color(&e.kind);
        ctx.set_stroke_style_str(edge_color);
        // Re-target the curve so it ends at the arrowhead base, not at
        // the chip centre — the head visually completes the line so
        // the original endpoint is no longer needed.
        ctx.begin_path();
        ctx.move_to(fx, fy);
        ctx.quadratic_curve_to(cx, cy, base_x, base_y);
        ctx.stroke();
        // Arrowhead: solid filled triangle in the same colour as the
        // stroke. Drop the dash pattern first so it doesn't bleed
        // into the head outline. Always solid even for REFERENCES.
        ctx.set_line_dash(&js_sys::Array::new()).ok();
        ctx.set_fill_style_str(edge_color);
        ctx.begin_path();
        ctx.move_to(tip_x, tip_y);
        ctx.line_to(left_x, left_y);
        ctx.line_to(right_x, right_y);
        ctx.close_path();
        ctx.fill();
    }
    ctx.set_line_dash(&js_sys::Array::new()).ok();
    ctx.set_global_alpha(1.0);
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
    ctx.set_text_align("center");
    ctx.set_text_baseline("middle");
    ctx.set_font("14px system-ui, sans-serif");
    let _ = ctx.fill_text(
        "Load a graph payload to render.",
        width * 0.5,
        height * 0.5,
    );
}

fn fill_round_rect(ctx: &CanvasRenderingContext2d, x: f64, y: f64, w: f64, h: f64, r: f64) {
    trace_round_rect(ctx, x, y, w, h, r);
    ctx.fill();
}

fn stroke_round_rect(ctx: &CanvasRenderingContext2d, x: f64, y: f64, w: f64, h: f64, r: f64) {
    trace_round_rect(ctx, x, y, w, h, r);
    ctx.stroke();
}

fn trace_round_rect(ctx: &CanvasRenderingContext2d, x: f64, y: f64, w: f64, h: f64, r: f64) {
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

/// World-space stroke width that renders as `screen_px` on screen at
/// the current viewport scale. Without this, lines hairline-disappear
/// when zoomed out and look chunky when zoomed in.
fn stroke_world_px(screen_px: f64, scale: f64) -> f64 {
    screen_px / scale.max(0.0001)
}

pub(crate) fn truncate_to_width(
    ctx: &CanvasRenderingContext2d,
    text: &str,
    max_width: f64,
) -> String {
    if let Ok(metrics) = ctx.measure_text(text) {
        if metrics.width() <= max_width {
            return text.to_string();
        }
    }
    // Binary search for the longest prefix that fits + `…`.
    let chars: Vec<char> = text.chars().collect();
    let mut lo = 0;
    let mut hi = chars.len();
    let mut best = String::new();
    while lo < hi {
        let mid = (lo + hi + 1) / 2;
        let mut candidate: String = chars[..mid].iter().collect();
        candidate.push('…');
        let fits = ctx
            .measure_text(&candidate)
            .map(|m| m.width() <= max_width)
            .unwrap_or(false);
        if fits {
            best = candidate;
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    if best.is_empty() {
        "…".into()
    } else {
        best
    }
}


//! Canvas 2D rendering. One entrypoint, [`draw`], called from
//! `GraphEngine::tick` once per frame. The renderer is stateless: it
//! reads scene + viewport + interaction state and paints — no
//! retained side effects between frames.

use web_sys::CanvasRenderingContext2d;

use crate::interaction::InteractionState;
use crate::palette::Palette;
use crate::scene::{Bounds, Node, Scene};
use crate::viewport::Viewport;

const CLUSTER_RADIUS: f64 = 6.0;
const CHIP_RADIUS: f64 = 4.0;
const FONT_PX: f64 = 12.0;
const HEADER_FONT_PX: f64 = 12.0;
/// Width (world units) of the per-chip language accent bar. Kept in
/// lock-step with `ACCENT_BAR_W` in `gpu/chip.wgsl`.
const ACCENT_BAR_W: f64 = 4.0;

// Minimap — a fixed-size overview panel pinned bottom-right, drawn in
// screen space after the world render. Project frames map into it as
// blips; the current viewport shows as an outlined rect; a click
// inside teleports the viewport (see `minimap_world_target`).
const MINIMAP_W: f64 = 220.0;
const MINIMAP_H: f64 = 150.0;
const MINIMAP_MARGIN: f64 = 12.0;
const MINIMAP_PAD: f64 = 8.0;

// LOD thresholds — scale ratios below which we skip increasingly
// expensive draw work. Without these, a "fit-everything" zoom on a
// 1 k+ symbol graph would draw 3 + N text labels per frame at sizes
// well below the eye's reading threshold, paying for invisible
// pixels. Numbers are scale = viewport.scale (1.0 = unzoomed).
const LOD_TEXT_MIN_SCALE: f64 = 0.5; // chip name + cluster title
const LOD_GLYPH_MIN_SCALE: f64 = 0.4; // kind glyph (fn / T / val …)
const LOD_OUTLINE_MIN_SCALE: f64 = 0.2; // chip stroke (rect outlines)

// Semantic-zoom tier boundaries. Below `PROJECT` the canvas paints
// only project blocks; between `PROJECT` and `MODULE` it paints the
// frame tree without chips; above `MODULE` it paints everything.
// Aggregating this way keeps a 5 k-chip overview legible instead of
// drowning it in sub-pixel rectangles.
const LOD_PROJECT_MAX_SCALE: f64 = 0.16;
const LOD_MODULE_MAX_SCALE: f64 = 0.46;

/// Granularity the canvas renders at the current viewport scale.
#[derive(Clone, Copy, PartialEq, Eq)]
enum LodTier {
    /// Zoomed out — project frames only, as solid blocks.
    Project,
    /// Mid range — the full frame tree (projects + modules), no chips.
    Module,
    /// Zoomed in — frames, sections and individual chips.
    Chip,
}

fn lod_tier(scale: f64) -> LodTier {
    if scale < LOD_PROJECT_MAX_SCALE {
        LodTier::Project
    } else if scale < LOD_MODULE_MAX_SCALE {
        LodTier::Module
    } else {
        LodTier::Chip
    }
}

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

    let tier = lod_tier(viewport.scale);
    match tier {
        LodTier::Project => {
            draw_project_blocks(ctx, scene, palette, viewport.scale, view_box);
            draw_frame_wires(ctx, scene, palette, viewport.scale, view_box, tier);
        }
        LodTier::Module => {
            draw_clusters(ctx, scene, palette, viewport.scale, view_box);
            draw_frame_wires(ctx, scene, palette, viewport.scale, view_box, tier);
        }
        LodTier::Chip => {
            draw_clusters(ctx, scene, palette, viewport.scale, view_box);
            draw_sections(ctx, scene, palette, viewport.scale, view_box);
            draw_nodes(ctx, scene, palette, viewport.scale, hovered_fqdn, view_box);
        }
    }
    if hovered_fqdn.is_some() {
        draw_edges_for_hovered(ctx, scene, palette, viewport.scale, hovered_fqdn);
    }

    // Minimap overlay — screen space, on top of everything.
    let _ = ctx.set_transform(dpr, 0.0, 0.0, dpr, 0.0, 0.0);
    draw_minimap(ctx, width, height, scene, viewport, palette);
}

/// Screen-space rect `(x, y, w, h)` of the minimap panel — pinned to
/// the bottom-right corner.
fn minimap_screen_rect(width: f64, height: f64) -> (f64, f64, f64, f64) {
    (
        width - MINIMAP_W - MINIMAP_MARGIN,
        height - MINIMAP_H - MINIMAP_MARGIN,
        MINIMAP_W,
        MINIMAP_H,
    )
}

/// World→minimap fit: maps `bounds` into the minimap's padded inner
/// box, centered. Returns `(scale, off_x, off_y)` such that a world
/// point projects via `mm = off + world * scale`.
fn minimap_transform(width: f64, height: f64, bounds: Bounds) -> Option<(f64, f64, f64)> {
    if !bounds.is_valid() {
        return None;
    }
    let (mx, my, _, _) = minimap_screen_rect(width, height);
    let inner_w = MINIMAP_W - MINIMAP_PAD * 2.0;
    let inner_h = MINIMAP_H - MINIMAP_PAD * 2.0;
    let scale = (inner_w / bounds.width().max(1.0)).min(inner_h / bounds.height().max(1.0));
    let off_x = mx + MINIMAP_PAD + (inner_w - bounds.width() * scale) * 0.5 - bounds.min_x * scale;
    let off_y = my + MINIMAP_PAD + (inner_h - bounds.height() * scale) * 0.5 - bounds.min_y * scale;
    Some((scale, off_x, off_y))
}

/// If screen point `(sx, sy)` lands inside the minimap panel, return
/// the world point it maps to — used for click-to-teleport. `None`
/// when the point is outside the panel.
pub(crate) fn minimap_world_target(
    width: f64,
    height: f64,
    bounds: Bounds,
    sx: f64,
    sy: f64,
) -> Option<(f64, f64)> {
    let (mx, my, mw, mh) = minimap_screen_rect(width, height);
    if sx < mx || sx > mx + mw || sy < my || sy > my + mh {
        return None;
    }
    let (scale, off_x, off_y) = minimap_transform(width, height, bounds)?;
    Some(((sx - off_x) / scale, (sy - off_y) / scale))
}

/// Paint the minimap: a panel, every project frame as a kind-coloured
/// blip, and the current viewport as an outlined rect. Clipped to the
/// panel so a viewbox panned past the graph bounds stays contained.
fn draw_minimap(
    ctx: &CanvasRenderingContext2d,
    width: f64,
    height: f64,
    scene: &Scene,
    viewport: &Viewport,
    palette: &Palette,
) {
    let (mx, my, mw, mh) = minimap_screen_rect(width, height);
    let Some((scale, off_x, off_y)) = minimap_transform(width, height, scene.bounds()) else {
        return;
    };

    ctx.set_global_alpha(0.92);
    ctx.set_fill_style_str(&palette.widget_background);
    fill_round_rect(ctx, mx, my, mw, mh, 4.0);
    ctx.set_global_alpha(1.0);
    ctx.set_stroke_style_str(&palette.panel_border);
    ctx.set_line_width(1.0);
    stroke_round_rect(ctx, mx, my, mw, mh, 4.0);

    ctx.save();
    trace_round_rect(ctx, mx, my, mw, mh, 4.0);
    ctx.clip();

    for n in &scene.hierarchy.nodes {
        let Some(kind) = n.project_kind.as_deref() else {
            continue;
        };
        ctx.set_fill_style_str(palette.project_color(kind));
        ctx.fill_rect(
            off_x + n.x as f64 * scale,
            off_y + n.y as f64 * scale,
            (n.w as f64 * scale).max(1.0),
            (n.h as f64 * scale).max(1.0),
        );
    }

    let (vx0, vy0) = viewport.screen_to_world(0.0, 0.0);
    let (vx1, vy1) = viewport.screen_to_world(width, height);
    ctx.set_stroke_style_str(&palette.focus_border);
    ctx.set_line_width(1.5);
    ctx.stroke_rect(
        off_x + vx0 * scale,
        off_y + vy0 * scale,
        (vx1 - vx0) * scale,
        (vy1 - vy0) * scale,
    );

    ctx.restore();
}

/// Project-tier render: every project frame as a solid kind-coloured
/// block stamped with its recursive symbol count. Module frames and
/// chips are omitted — the overview answers "which projects, how big"
/// without drowning in thousands of chip rectangles. Root frames with
/// no project (the `(unscoped)` bucket) paint as a neutral block.
fn draw_project_blocks(
    ctx: &CanvasRenderingContext2d,
    scene: &Scene,
    palette: &Palette,
    scale: f64,
    view_box: ViewportBox,
) {
    ctx.set_stroke_style_str(&palette.panel_border);
    ctx.set_line_width(stroke_world_px(1.5, scale));
    for n in &scene.hierarchy.nodes {
        if !is_project_block(n) {
            continue;
        }
        let (x, y, w, h) = (n.x as f64, n.y as f64, n.w as f64, n.h as f64);
        if !view_box.intersects_rect(x, y, w, h) {
            continue;
        }
        let fill = match n.project_kind.as_deref() {
            Some(kind) => palette.project_color(kind),
            None => &palette.widget_background,
        };
        ctx.set_fill_style_str(fill);
        fill_round_rect(ctx, x, y, w, h, CLUSTER_RADIUS);
        stroke_round_rect(ctx, x, y, w, h, CLUSTER_RADIUS);
    }

    // Label + count, sized in world units so they land at a fixed
    // on-screen size whatever the overview zoom (world px = target /
    // scale). Drawn last so the text sits above any nested block.
    let title_px = 15.0 / scale.max(0.0001);
    let count_px = 12.0 / scale.max(0.0001);
    ctx.set_text_align("left");
    ctx.set_text_baseline("alphabetic");
    for n in &scene.hierarchy.nodes {
        if !is_project_block(n) {
            continue;
        }
        let (x, y, w, h) = (n.x as f64, n.y as f64, n.w as f64, n.h as f64);
        if !view_box.intersects_rect(x, y, w, h) {
            continue;
        }
        ctx.set_fill_style_str(&palette.foreground);
        ctx.set_font(&format!("600 {title_px}px system-ui, sans-serif"));
        let _ = ctx.fill_text(&n.segment, x + title_px * 0.5, y + title_px * 1.1);
        ctx.set_fill_style_str(&palette.description);
        ctx.set_font(&format!("{count_px}px system-ui, sans-serif"));
        let _ = ctx.fill_text(
            &format!("{} symbols", n.recursive_symbol_count),
            x + title_px * 0.5,
            y + title_px * 1.1 + count_px * 1.3,
        );
    }
}

/// A hierarchy node painted as a solid block at the project LOD tier:
/// any project frame, plus root frames (the `(unscoped)` bucket).
fn is_project_block(n: &crate::hierarchy::HierarchyNode) -> bool {
    n.project_kind.is_some() || n.parent.is_none()
}

/// Persistent dependency wires between sibling frames. A wire runs
/// from the foundation frame's right edge to the dependant frame's
/// left edge (the layout places the dependant one column right) — a
/// flat Bézier with an arrowhead, the UE-Blueprint pin link. Drawn at
/// the project + module tiers; the chip tier keeps the hover edges.
fn draw_frame_wires(
    ctx: &CanvasRenderingContext2d,
    scene: &Scene,
    palette: &Palette,
    scale: f64,
    view_box: ViewportBox,
    tier: LodTier,
) {
    if scene.frame_edges.is_empty() {
        return;
    }
    ctx.set_stroke_style_str(&palette.text_link);
    ctx.set_fill_style_str(&palette.text_link);
    ctx.set_global_alpha(0.5);
    ctx.set_line_width(stroke_world_px(1.6, scale));
    for &(a, b) in &scene.frame_edges {
        let (Some(na), Some(nb)) = (
            scene.hierarchy.nodes.get(a as usize),
            scene.hierarchy.nodes.get(b as usize),
        ) else {
            continue;
        };
        // The project tier paints only project-block frames — skip a
        // wire unless both its ends are visible there.
        if tier == LodTier::Project && !(is_project_block(na) && is_project_block(nb)) {
            continue;
        }
        let (ax, ay) = (na.x as f64 + na.w as f64, na.y as f64 + na.h as f64 * 0.5);
        let (bx, by) = (nb.x as f64, nb.y as f64 + nb.h as f64 * 0.5);
        let (min_x, min_y) = (ax.min(bx), ay.min(by));
        if !view_box.intersects_rect(min_x, min_y, (ax - bx).abs(), (ay - by).abs()) {
            continue;
        }
        // Flat S-curve — control points pulled horizontally so the
        // wire leaves the foundation rightward and enters the
        // dependant from the left.
        let pull = ((bx - ax).abs() * 0.5).max(48.0);
        ctx.begin_path();
        ctx.move_to(ax, ay);
        ctx.bezier_curve_to(ax + pull, ay, bx - pull, by, bx, by);
        ctx.stroke();
        // Arrowhead at the dependant end — direction ≈ tangent at the
        // curve end, collinear with `(end − control2)`.
        let (dx, dy) = (bx - (bx - pull), by - by);
        let len = dx.hypot(dy);
        if len < 0.1 {
            continue;
        }
        let (ux, uy) = (dx / len, dy / len);
        let head = 9.0 / scale.max(0.0001);
        let (basex, basey) = (bx - ux * head, by - uy * head);
        let half = head * 0.5;
        ctx.begin_path();
        ctx.move_to(bx, by);
        ctx.line_to(basex - uy * half, basey + ux * half);
        ctx.line_to(basex + uy * half, basey - ux * half);
        ctx.close_path();
        ctx.fill();
    }
    ctx.set_global_alpha(1.0);
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

    // Project frames get a kind-coloured header band. One node per
    // project (a dozen at most), so clipping to the container's
    // rounded silhouette per node is free — and it gives the band
    // crisp rounded top corners with a flush square bottom edge.
    let band_h = crate::layout::CONTAINER_HEADER_H as f64;
    for n in &scene.hierarchy.nodes {
        let Some(kind) = n.project_kind.as_deref() else {
            continue;
        };
        let (x, y, w, h) = (n.x as f64, n.y as f64, n.w as f64, n.h as f64);
        if !view_box.intersects_rect(x, y, w, h) {
            continue;
        }
        ctx.save();
        trace_round_rect(ctx, x, y, w, h, CLUSTER_RADIUS);
        ctx.clip();
        ctx.set_fill_style_str(palette.project_color(kind));
        ctx.fill_rect(x, y, w, band_h.min(h));
        ctx.restore();
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
        // Title (segment) — top line. Project frames sit on a
        // coloured band, so their title takes `foreground` for
        // contrast; module nodes keep the `text_link` accent.
        let title_color = if n.project_kind.is_some() {
            &palette.foreground
        } else {
            &palette.text_link
        };
        ctx.set_font(&header_font);
        ctx.set_fill_style_str(title_color);
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
        // On a project band the muted `description` grey washes out —
        // reuse the title's contrast colour there.
        ctx.set_fill_style_str(if n.project_kind.is_some() {
            &palette.foreground
        } else {
            &palette.description
        });
        let _ = ctx.fill_text(
            &format!("({count})"),
            x + 12.0 + title_width + 6.0,
            y + 14.0,
        );

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
            ctx.set_line_dash(&js_sys::Array::of2(&3.0.into(), &2.0.into()))
                .ok();
        }
        stroke_round_rect(ctx, n.x, n.y, n.w, n.h, CHIP_RADIUS);
        if n.is_external {
            ctx.set_global_alpha(1.0);
            ctx.set_line_dash(&js_sys::Array::new()).ok();
        }
    }

    // Left accent bar — the chip's source language at a glance. Drawn
    // after the outline so it sits flush over the left hairline.
    // Inset vertically by the corner radius so it stays inside the
    // rounded silhouette without paying for a per-chip clip path.
    ctx.set_fill_style_str(palette.language_color(&n.language));
    ctx.fill_rect(n.x, n.y + CHIP_RADIUS, ACCENT_BAR_W, n.h - 2.0 * CHIP_RADIUS);

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
    let Some(&from_idx) = scene.node_by_fqdn.get(fqdn) else {
        return;
    };
    ctx.set_line_width(stroke_world_px(2.0, scale));
    ctx.set_global_alpha(0.85);
    for e in &scene.edges {
        let touches = e.from_node == from_idx || e.to_node == from_idx;
        if !touches {
            continue;
        }
        let Some(from) = scene.nodes.get(e.from_node) else {
            continue;
        };
        let Some(to) = scene.nodes.get(e.to_node) else {
            continue;
        };
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
            ctx.set_line_dash(&js_sys::Array::of2(&4.0.into(), &3.0.into()))
                .ok();
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
    let _ = ctx.fill_text("Load a graph payload to render.", width * 0.5, height * 0.5);
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
    if best.is_empty() { "…".into() } else { best }
}

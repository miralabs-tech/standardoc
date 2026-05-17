//! 2D affine viewport: scale + translate. World → screen is
//! `screen = world * scale + offset`. The renderer applies the
//! resulting transform once per frame via
//! `CanvasRenderingContext2d::set_transform`, so node draw code can
//! work in world units without per-call multiplications.

use crate::scene::Bounds;

const MIN_SCALE: f64 = 0.05;
const MAX_SCALE: f64 = 8.0;

#[derive(Debug, Clone, Copy)]
pub(crate) struct Viewport {
    pub scale: f64,
    pub offset_x: f64,
    pub offset_y: f64,
}

impl Viewport {
    pub(crate) const fn identity() -> Self {
        Self {
            scale: 1.0,
            offset_x: 0.0,
            offset_y: 0.0,
        }
    }

    pub(crate) fn pan(&mut self, dx: f64, dy: f64) {
        self.offset_x += dx;
        self.offset_y += dy;
    }

    /// Zoom around a screen-space pivot so the world point under the
    /// cursor stays put. Standard "zoom toward cursor" formula.
    pub(crate) fn zoom_around(&mut self, screen_x: f64, screen_y: f64, factor: f64) {
        let next_scale = (self.scale * factor).clamp(MIN_SCALE, MAX_SCALE);
        let k = next_scale / self.scale;
        self.offset_x = screen_x - k * (screen_x - self.offset_x);
        self.offset_y = screen_y - k * (screen_y - self.offset_y);
        self.scale = next_scale;
    }

    pub(crate) fn set_scale(&mut self, scale: f64) {
        self.scale = scale.clamp(MIN_SCALE, MAX_SCALE);
    }

    pub(crate) fn screen_to_world(&self, sx: f64, sy: f64) -> (f64, f64) {
        (
            (sx - self.offset_x) / self.scale,
            (sy - self.offset_y) / self.scale,
        )
    }

    /// Pick scale + offset such that `bounds` fits in `viewport_w *
    /// viewport_h`, centered, with ~5% margin.
    pub(crate) fn fit_to(&mut self, bounds: Bounds, viewport_w: f64, viewport_h: f64) {
        if !bounds.is_valid() || viewport_w <= 0.0 || viewport_h <= 0.0 {
            *self = Self::identity();
            return;
        }
        let scale_x = viewport_w / bounds.width().max(1.0);
        let scale_y = viewport_h / bounds.height().max(1.0);
        let scale = (scale_x.min(scale_y) * 0.95).clamp(MIN_SCALE, MAX_SCALE);
        self.scale = scale;
        self.center_on(bounds, viewport_w, viewport_h);
    }

    /// Keep the current scale, just slide the viewport so `bounds` is
    /// centered. Used on resize, so the user doesn't lose their
    /// zoom level when the panel changes size.
    pub(crate) fn center_on(&mut self, bounds: Bounds, viewport_w: f64, viewport_h: f64) {
        if !bounds.is_valid() {
            return;
        }
        let cx = (bounds.min_x + bounds.max_x) * 0.5;
        let cy = (bounds.min_y + bounds.max_y) * 0.5;
        self.offset_x = viewport_w * 0.5 - cx * self.scale;
        self.offset_y = viewport_h * 0.5 - cy * self.scale;
    }
}

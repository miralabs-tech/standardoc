// This crate is wasm-only by design: the renderer drives a browser
// canvas (Canvas2D + WebGPU) and the public API is wired through
// `wasm-bindgen`. Gating the whole crate behind `wasm32` keeps
// `cargo check --workspace` (which defaults to the host target) green
// — without this guard, native compilation trips on `wgpu`'s
// `SurfaceTarget::Canvas` variant (`#[cfg(web)]` upstream), which has
// no native counterpart.
#![cfg(target_arch = "wasm32")]

//! WASM graph viewer for the standardoc semantic IR.
//!
//! Phase 3 split-canvas architecture: the host shell composes two
//! independent canvases.
//!
//! - [`overview::OverviewCanvas`] — workspace-level nebula. Projects
//!   live in 3D world space; a shared [`camera::Camera3D`] orbits the
//!   centroid; clusters depth-sort back-to-front each frame.
//! - [`focus::FocusGraphCanvas`] — symbol-local view. Focal at centre,
//!   BFS-N neighbours fan out across concentric rings, edges labelled
//!   inline via a DOM overlay.
//!
//! The legacy `GraphEngine` (single canvas + drill tree) lived here
//! through Phases 1–3c. Phase 3d removed it along with its tree /
//! scene / render / layout / viewport / interaction modules — the new
//! split canvases supersede it.

#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_precision_loss)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::cast_lossless)]
// `palette` and `payload` still carry helpers that used to feed the
// legacy GraphEngine; the slim canvases consume only a subset. The
// crate-wide allow keeps the slim-down focused on deletion rather than
// another bookkeeping pass — the next time one of these helpers grows
// a real consumer the lint will fire.
#![allow(dead_code)]

mod camera;
mod focus;
mod kind;
mod overview;
mod palette;
mod payload;

use wasm_bindgen::prelude::*;
use web_sys::HtmlCanvasElement;

/// Called automatically the first time `init()` runs on the JS side.
/// Installs a panic hook that pipes Rust panics into the browser
/// console with full backtraces — without it, panics show up as
/// `unreachable executed` which is useless to debug.
#[wasm_bindgen(start)]
pub fn _wasm_start() {
    console_error_panic_hook::set_once();
}

/// Resize a host-owned canvas. The bitmap (backing store) is set to
/// `width * dpr × height * dpr` for sharp HiDPI rendering; the CSS
/// (intrinsic) size is pinned to the logical `width × height` so the
/// canvas does NOT advertise its DPR-scaled backing size as its
/// layout size. Without that pin, Chrome treats the bitmap size as
/// the intrinsic size and grows the parent flex container, which the
/// `ResizeObserver` immediately observes — yielding a runaway loop
/// where every resize tick makes the canvas a few pixels bigger.
pub(crate) fn apply_canvas_size(canvas: &HtmlCanvasElement, width: u32, height: u32, dpr: f64) {
    canvas.set_width((f64::from(width) * dpr) as u32);
    canvas.set_height((f64::from(height) * dpr) as u32);
    let style = canvas.style();
    let _ = style.set_property("width", &format!("{width}px"));
    let _ = style.set_property("height", &format!("{height}px"));
}

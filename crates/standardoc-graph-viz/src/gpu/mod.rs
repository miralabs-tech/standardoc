//! WebGPU render backend, hybrid sibling of the Canvas2D path in
//! [`crate::render`]. Activated by [`crate::GraphEngine::enable_webgpu`].
//!
//! Design pillars:
//! - **Single draw call per frame** via instanced quads. The vertex
//!   buffer holds one unit quad shared across every chip; the
//!   instance buffer holds one row per chip (position, size, colors,
//!   corner radius, stroke width). 1 k symbols = 1 draw call.
//! - **SDF rounded corners + stroke** in the fragment shader, so the
//!   appearance matches Canvas2D without the per-chip path-building
//!   cost. `fwidth(d)` keeps the AA band crisp under any zoom.
//! - **Static bindings only**. wgpu's web target speaks WebGPU via
//!   `web_sys::Gpu*` typed APIs — there is no `js_sys::Reflect` in
//!   the hot path, fulfilling the project's "no introspection" rule.

mod backend;

pub(crate) use backend::{LevelNode, WebGpuBackend};

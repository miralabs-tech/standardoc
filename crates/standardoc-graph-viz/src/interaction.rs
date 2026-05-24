//! Pointer interaction state. Kept in its own module so the engine's
//! public methods don't have to thread tuples around — they push
//! events at this struct and consult it from the renderer.

#[derive(Debug, Default)]
pub(crate) struct InteractionState {
    /// Fqdn of the hovered leaf card — drives the host's
    /// `sd-graph-hover` callback and the 2D edge fade. Always `None`
    /// for hovered container cards (they carry an empty fqdn);
    /// `hovered_tree_idx` covers the universal-hover use cases.
    hovered: Option<String>,
    /// `DrillTree.nodes` index of the hovered card (leaf OR
    /// container). Drives the 3D label layer's hover-only filter so
    /// only the focused node's text floats over the canvas instead
    /// of every label at once.
    hovered_tree_idx: Option<u32>,
    pan_origin: Option<(f64, f64)>,
    /// `(tree_idx, down_x, down_y)` — set on `on_pointer_down`,
    /// consumed on `on_pointer_up`. `tree_idx` is the
    /// `DrillTree.nodes` index of the picked card (stable across
    /// renders, unlike per-frame card indices). Used to distinguish
    /// click vs drag: a click is recognised only when the up
    /// position is within ~5 px of the down position.
    click_candidate: Option<(u32, f64, f64)>,
}

impl InteractionState {
    pub(crate) fn hovered(&self) -> Option<String> {
        self.hovered.clone()
    }

    pub(crate) fn hovered_ref(&self) -> Option<&str> {
        self.hovered.as_deref()
    }

    pub(crate) fn set_hovered(&mut self, fqdn: Option<String>) {
        self.hovered = fqdn;
    }

    pub(crate) const fn hovered_tree_idx(&self) -> Option<u32> {
        self.hovered_tree_idx
    }

    pub(crate) fn set_hovered_tree_idx(&mut self, idx: Option<u32>) {
        self.hovered_tree_idx = idx;
    }

    pub(crate) const fn pan_origin(&self) -> Option<(f64, f64)> {
        self.pan_origin
    }

    pub(crate) fn set_pan_origin(&mut self, origin: Option<(f64, f64)>) {
        self.pan_origin = origin;
    }

    pub(crate) fn set_click_candidate(&mut self, candidate: Option<(u32, f64, f64)>) {
        self.click_candidate = candidate;
    }

    pub(crate) fn take_click_candidate(&mut self) -> Option<(u32, f64, f64)> {
        self.click_candidate.take()
    }
}

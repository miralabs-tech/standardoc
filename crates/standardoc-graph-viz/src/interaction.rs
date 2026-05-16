//! Pointer interaction state. Kept in its own module so the engine's
//! public methods don't have to thread tuples around — they push
//! events at this struct and consult it from the renderer.

#[derive(Debug, Default)]
pub(crate) struct InteractionState {
    hovered: Option<String>,
    pan_origin: Option<(f64, f64)>,
    /// `(fqdn, down_x, down_y)` — set on `on_pointer_down`, consumed
    /// on `on_pointer_up`. Used to distinguish click vs drag: a click
    /// is recognised only when the up position is within ~5 px of the
    /// down position.
    click_candidate: Option<(String, f64, f64)>,
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

    pub(crate) const fn pan_origin(&self) -> Option<(f64, f64)> {
        self.pan_origin
    }

    pub(crate) fn set_pan_origin(&mut self, origin: Option<(f64, f64)>) {
        self.pan_origin = origin;
    }

    pub(crate) fn set_click_candidate(&mut self, candidate: Option<(String, f64, f64)>) {
        self.click_candidate = candidate;
    }

    pub(crate) fn take_click_candidate(&mut self) -> Option<(String, f64, f64)> {
        self.click_candidate.take()
    }
}

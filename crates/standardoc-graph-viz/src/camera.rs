//! Orbit camera for the WebGPU 3D path. The Canvas2D path keeps its
//! own 2D affine [`crate::viewport::Viewport`]; this is the perspective
//! sibling — a `target` point the camera orbits, plus spherical
//! (`yaw` / `pitch` / `distance`) eye placement and a perspective
//! projection.

use glam::{Mat4, Vec3};

const MIN_PITCH: f32 = -1.45;
const MAX_PITCH: f32 = 1.45;
const MIN_DISTANCE: f32 = 10.0;
/// Hard upper bound on the orbit distance. Without it, holding the
/// keyboard backward key (S) for a few seconds at 1.5% / frame growth
/// drove `distance` past 10⁶ world units, well outside the `far`
/// plane — the geometry vanished and only clicking Home could recover.
/// 30_000 is ~10× the typical workspace fit distance (≈3000 for a
/// 1000-unit bbox); past this the scene is a sub-pixel dot anyway,
/// so capping here also keeps the camera in a "scene is meaningful"
/// regime.
const MAX_DISTANCE: f32 = 30_000.0;
/// Half-radius of the "pan playground" — target.length() is soft-
/// clamped to this in `pan()`. Capped at a multiple of `distance` so
/// dollying out gives more pan room without ever letting the camera
/// drift completely off the scene anchor.
const TARGET_CAP_DISTANCE_FACTOR: f32 = 3.0;
/// Screen-pixel → radian gain for drag-to-orbit.
const ORBIT_SPEED: f32 = 0.008;
/// Preset-transition progress gained per frame (~0.5 s at 60 fps).
const ANIM_SPEED: f32 = 1.0 / 30.0;

/// In-flight transition — start/end values for all four orbit
/// parameters plus a 0..1 progress the render loop advances once per
/// frame. Presets typically leave target / distance unchanged (from
/// equals to), while `frame()` typically leaves yaw / pitch unchanged.
#[derive(Debug, Clone, Copy)]
struct CameraAnim {
    from_yaw: f32,
    from_pitch: f32,
    to_yaw: f32,
    to_pitch: f32,
    from_target: Vec3,
    to_target: Vec3,
    from_distance: f32,
    to_distance: f32,
    t: f32,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Camera3D {
    /// World-space point the camera looks at and orbits around.
    pub target: Vec3,
    /// Distance from `target` to the eye.
    pub distance: f32,
    /// Azimuth around the vertical axis, radians.
    pub yaw: f32,
    /// Elevation, radians. Clamped shy of the poles to dodge the
    /// gimbal flip a straight-up/down look would cause.
    pub pitch: f32,
    /// Vertical field of view, radians.
    pub fov_y: f32,
    pub near: f32,
    pub far: f32,
    /// Active transition, advanced by `step_animation`.
    anim: Option<CameraAnim>,
    /// `true` once `frame()` has positioned the camera at least once.
    /// Subsequent `frame()` calls then animate instead of snapping —
    /// the very first call (scene load) snaps so the user lands on
    /// the geometry without an awkward fly-in from the identity pose.
    positioned: bool,
}

impl Camera3D {
    /// A neutral 3/4 view. `frame` overwrites `target` / `distance` /
    /// `near` / `far` once a scene is loaded; the angles persist.
    pub(crate) fn identity() -> Self {
        Self {
            target: Vec3::ZERO,
            distance: 1000.0,
            yaw: 0.7,
            pitch: 0.5,
            fov_y: 50.0_f32.to_radians(),
            near: 1.0,
            far: 100_000.0,
            anim: None,
            positioned: false,
        }
    }

    /// Eye position derived from the spherical orbit parameters.
    pub(crate) fn eye(&self) -> Vec3 {
        let cp = self.pitch.cos();
        let dir = Vec3::new(cp * self.yaw.sin(), self.pitch.sin(), cp * self.yaw.cos());
        self.target + dir * self.distance
    }

    /// View matrix — world space → camera space. Up is `-Y` so the
    /// vertical sense stays consistent with the screen.
    pub(crate) fn view(&self) -> Mat4 {
        Mat4::look_at_rh(self.eye(), self.target, Vec3::NEG_Y)
    }

    /// Perspective projection matrix for the given viewport aspect.
    pub(crate) fn proj(&self, aspect: f32) -> Mat4 {
        let aspect = if aspect.is_finite() && aspect > 0.0 {
            aspect
        } else {
            1.0
        };
        Mat4::perspective_rh(self.fov_y, aspect, self.near, self.far)
    }

    /// Drag-to-orbit. `dx` / `dy` are screen-pixel deltas. A manual
    /// drag cancels any in-flight transition.
    pub(crate) fn orbit(&mut self, dx: f64, dy: f64) {
        self.anim = None;
        self.yaw -= dx as f32 * ORBIT_SPEED;
        self.pitch = (self.pitch + dy as f32 * ORBIT_SPEED).clamp(MIN_PITCH, MAX_PITCH);
    }

    /// Begin an animated transition to a named preset angle. Unknown
    /// names are ignored. Presets only re-aim the orbit — `target`
    /// and `distance` are left untouched (from equals to).
    pub(crate) fn apply_preset(&mut self, name: &str) {
        let (to_yaw, to_pitch) = match name {
            "orbit" => (0.7, 0.5),
            "top" => (0.0, MAX_PITCH),
            "front" => (0.0, 0.0),
            "side" => (std::f32::consts::FRAC_PI_2, 0.0),
            _ => return,
        };
        self.anim = Some(CameraAnim {
            from_yaw: self.yaw,
            from_pitch: self.pitch,
            to_yaw,
            to_pitch,
            from_target: self.target,
            to_target: self.target,
            from_distance: self.distance,
            to_distance: self.distance,
            t: 0.0,
        });
    }

    /// `true` while a transition is still running.
    pub(crate) fn animating(&self) -> bool {
        self.anim.is_some()
    }

    /// Advance an in-flight transition by one frame, easing yaw,
    /// pitch, target and distance with a smoothstep. Recomputes
    /// near/far from the eased distance so the depth range tracks the
    /// fit. No-op when nothing is animating.
    pub(crate) fn step_animation(&mut self) {
        let Some(anim) = &mut self.anim else {
            return;
        };
        anim.t = (anim.t + ANIM_SPEED).min(1.0);
        let e = anim.t * anim.t * (3.0 - 2.0 * anim.t);
        self.yaw = anim.from_yaw + (anim.to_yaw - anim.from_yaw) * e;
        self.pitch = anim.from_pitch + (anim.to_pitch - anim.from_pitch) * e;
        self.target = anim.from_target + (anim.to_target - anim.from_target) * e;
        self.distance = anim.from_distance + (anim.to_distance - anim.from_distance) * e;
        self.near = (self.distance * 0.01).max(0.5);
        self.far = self.distance * 50.0;
        if anim.t >= 1.0 {
            self.anim = None;
        }
    }

    /// Wheel-to-dolly. `factor > 1` pulls the eye toward `target`;
    /// `factor < 1` pushes the eye away. A manual dolly cancels any
    /// in-flight transition for the same reason `orbit` does.
    /// `near` / `far` track `distance` so the frustum always covers
    /// the geometry — without this, sustained keyboard backward held
    /// the camera past the far plane and the scene rendered empty
    /// until a preset / Home click ran `frame()` to recompute it.
    pub(crate) fn dolly(&mut self, factor: f32) {
        self.anim = None;
        self.distance = (self.distance / factor).clamp(MIN_DISTANCE, MAX_DISTANCE);
        self.near = (self.distance * 0.01).max(0.5);
        self.far = self.distance * 50.0;
    }

    /// Drag-to-pan. `dx` / `dy` are screen-pixel deltas; `viewport_h`
    /// is the canvas height in CSS pixels (needed to convert screen
    /// pixels to a world distance at the current camera distance).
    /// Translates `target` in the camera's right/up plane so the world
    /// point under the cursor stays under the cursor (Figma-style grab
    /// semantics). A manual pan cancels any in-flight transition.
    ///
    /// `target` is soft-clamped to a sphere of radius `distance * 10`
    /// around the origin: clusters live near the origin in our world
    /// space, and without the clamp, sustained keyboard pan (A/Q/E/D
    /// held) drifted the target tens of thousands of units away. The
    /// scene would still render but appear as a tiny dot at the edge
    /// of the viewport, indistinguishable from a "stuck camera" bug.
    pub(crate) fn pan(&mut self, dx: f64, dy: f64, viewport_h: f64) {
        self.anim = None;
        let viewport_h = (viewport_h as f32).max(1.0);
        let world_per_pixel = self.distance * (self.fov_y * 0.5).tan() * 2.0 / viewport_h;
        let cp = self.pitch.cos();
        let forward = Vec3::new(cp * self.yaw.sin(), self.pitch.sin(), cp * self.yaw.cos()).normalize_or_zero();
        let right = forward.cross(Vec3::NEG_Y).normalize_or_zero();
        let up = right.cross(forward).normalize_or_zero();
        // Grab semantics: target shifts OPPOSITE to the drag so the
        // world point under the cursor stays put. dx>0 (drag right) ⇒
        // target moves -right; dy>0 (drag down on screen ⇒ +Y world
        // ⇒ -up in our up-is-NEG_Y frame) ⇒ target moves -up.
        self.target -= right * (dx as f32 * world_per_pixel) + up * (dy as f32 * world_per_pixel);
        // Soft cap on target wander — keep the camera within a sane
        // neighbourhood of the scene origin even if a key is held for
        // a long time. Scaled by distance so dollying out gives more
        // pan room, but capped tightly enough that we never lose the
        // scene off-frame.
        let cap = self.distance * TARGET_CAP_DISTANCE_FACTOR;
        let target_norm = self.target.length();
        if target_norm > cap {
            self.target *= cap / target_norm;
        }
    }

    /// Frame a bounding sphere (`center` + `radius`) so the whole
    /// point cloud fits with a small margin. The first call snaps so
    /// the user lands on geometry immediately; subsequent calls
    /// animate the target / distance change so scope drills read as a
    /// smooth fly-to rather than a jarring snap.
    pub(crate) fn frame(&mut self, center: Vec3, radius: f32) {
        let radius = radius.max(1.0);
        let fit = radius / (self.fov_y * 0.5).tan();
        let to_distance = (fit * 1.4).max(MIN_DISTANCE);
        let to_far = to_distance * 50.0 + radius;
        if !self.positioned {
            self.target = center;
            self.distance = to_distance;
            self.near = (self.distance * 0.01).max(0.5);
            self.far = to_far;
            self.positioned = true;
            return;
        }
        // Snap near/far up-front so the depth range covers both the
        // current and target geometry while the ease runs.
        self.near = (self.distance.min(to_distance) * 0.01).max(0.5);
        self.far = self.far.max(to_far);
        self.anim = Some(CameraAnim {
            from_yaw: self.yaw,
            from_pitch: self.pitch,
            to_yaw: self.yaw,
            to_pitch: self.pitch,
            from_target: self.target,
            to_target: center,
            from_distance: self.distance,
            to_distance,
            t: 0.0,
        });
    }
}

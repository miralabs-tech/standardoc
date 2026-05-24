//! Orbit camera for the WebGPU 3D path. The Canvas2D path keeps its
//! own 2D affine [`crate::viewport::Viewport`]; this is the perspective
//! sibling — a `target` point the camera orbits, plus spherical
//! (`yaw` / `pitch` / `distance`) eye placement and a perspective
//! projection.

use glam::{Mat4, Vec3};

const MIN_PITCH: f32 = -1.45;
const MAX_PITCH: f32 = 1.45;
const MIN_DISTANCE: f32 = 10.0;
/// Screen-pixel → radian gain for drag-to-orbit.
const ORBIT_SPEED: f32 = 0.008;
/// Preset-transition progress gained per frame (~0.5 s at 60 fps).
const ANIM_SPEED: f32 = 1.0 / 30.0;

/// In-flight preset transition — start/end angles and a 0..1 progress
/// the render loop advances once per frame.
#[derive(Debug, Clone, Copy)]
struct CameraAnim {
    from_yaw: f32,
    from_pitch: f32,
    to_yaw: f32,
    to_pitch: f32,
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
    /// Active preset transition, advanced by `step_animation`.
    anim: Option<CameraAnim>,
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
    /// drag cancels any in-flight preset transition.
    pub(crate) fn orbit(&mut self, dx: f64, dy: f64) {
        self.anim = None;
        self.yaw -= dx as f32 * ORBIT_SPEED;
        self.pitch = (self.pitch + dy as f32 * ORBIT_SPEED).clamp(MIN_PITCH, MAX_PITCH);
    }

    /// Begin an animated transition to a named preset angle. Unknown
    /// names are ignored. Presets only re-aim the orbit — `target`
    /// and `distance` are left untouched.
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
            t: 0.0,
        });
    }

    /// `true` while a preset transition is still running.
    pub(crate) fn animating(&self) -> bool {
        self.anim.is_some()
    }

    /// Advance an in-flight preset transition by one frame, easing the
    /// yaw/pitch with a smoothstep. No-op when nothing is animating.
    pub(crate) fn step_animation(&mut self) {
        let Some(anim) = &mut self.anim else {
            return;
        };
        anim.t = (anim.t + ANIM_SPEED).min(1.0);
        let e = anim.t * anim.t * (3.0 - 2.0 * anim.t);
        self.yaw = anim.from_yaw + (anim.to_yaw - anim.from_yaw) * e;
        self.pitch = anim.from_pitch + (anim.to_pitch - anim.from_pitch) * e;
        if anim.t >= 1.0 {
            self.anim = None;
        }
    }

    /// Wheel-to-dolly. `factor > 1` pulls the eye toward `target`.
    pub(crate) fn dolly(&mut self, factor: f32) {
        self.distance = (self.distance / factor).max(MIN_DISTANCE);
    }

    /// Frame a bounding sphere (`center` + `radius`) so the whole
    /// point cloud fits with a small margin. `near` / `far` are
    /// derived from the fitted distance to keep the depth range tight
    /// around the geometry.
    pub(crate) fn frame(&mut self, center: Vec3, radius: f32) {
        let radius = radius.max(1.0);
        self.target = center;
        let fit = radius / (self.fov_y * 0.5).tan();
        self.distance = (fit * 1.4).max(MIN_DISTANCE);
        self.near = (self.distance * 0.01).max(0.5);
        self.far = self.distance * 50.0 + radius;
    }
}

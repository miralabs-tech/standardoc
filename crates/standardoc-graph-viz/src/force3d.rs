//! Per-level force-directed layout for the WebGPU drill-down view.
//!
//! Each drill level shows only the focused node's direct children — a
//! bounded set (the 13 projects, or the members of one container).
//! This lays that set out in 3D: every node repels every other, the
//! level's aggregated edges pull their endpoints together, and a weak
//! gravity keeps the cloud centred. `step` runs once per frame from
//! `tick` until the layout settles, then freezes.

use glam::Vec3;

const REPULSION: f32 = 90_000.0;
const SPRING_K: f32 = 0.06;
const SPRING_LEN: f32 = 260.0;
const GRAVITY: f32 = 0.012;
const DAMPING: f32 = 0.85;
/// Per-step displacement clamp — stops an overlapping seed pair from
/// firing a node off-screen.
const MAX_SPEED: f32 = 90.0;
const MAX_ITERATIONS: u32 = 500;
/// Mean per-node kinetic energy below which the layout freezes.
const SETTLE_ENERGY: f32 = 0.5;
const EPSILON: f32 = 1.0e-3;

pub(crate) struct Force3D {
    positions: Vec<Vec3>,
    velocities: Vec<Vec3>,
    /// Spring set — index pairs into `positions`.
    edges: Vec<(u32, u32)>,
    iterations: u32,
    settled: bool,
}

impl Force3D {
    /// An empty, already-settled layout — the state before a graph
    /// loads or when a level has no children.
    pub(crate) fn empty() -> Self {
        Self {
            positions: Vec::new(),
            velocities: Vec::new(),
            edges: Vec::new(),
            iterations: 0,
            settled: true,
        }
    }

    /// Lay out one drill level: `n` sibling nodes joined by `edges`
    /// (index pairs into the sibling set). Positions seed on a sphere
    /// so the first frame already looks three-dimensional.
    pub(crate) fn for_level(n: usize, edges: Vec<(u32, u32)>) -> Self {
        if n == 0 {
            return Self::empty();
        }
        Self {
            positions: seed_sphere(n),
            velocities: vec![Vec3::ZERO; n],
            edges,
            iterations: 0,
            settled: false,
        }
    }

    pub(crate) fn settled(&self) -> bool {
        self.settled
    }

    pub(crate) fn positions(&self) -> &[Vec3] {
        &self.positions
    }

    /// Centroid and enclosing radius of the cloud — the camera frames
    /// its orbit from this.
    pub(crate) fn bounding_sphere(&self) -> (Vec3, f32) {
        if self.positions.is_empty() {
            return (Vec3::ZERO, SPRING_LEN);
        }
        let mut center = Vec3::ZERO;
        for p in &self.positions {
            center += *p;
        }
        center /= self.positions.len() as f32;
        let mut radius = 0.0_f32;
        for p in &self.positions {
            radius = radius.max((*p - center).length());
        }
        (center, radius.max(SPRING_LEN))
    }

    /// Advance the simulation one iteration.
    pub(crate) fn step(&mut self) {
        if self.settled {
            return;
        }
        let n = self.positions.len();
        let mut forces = vec![Vec3::ZERO; n];

        // Coulomb repulsion — all pairs (the level is bounded, so the
        // O(n²) form is fine).
        for i in 0..n {
            for j in (i + 1)..n {
                let delta = self.positions[i] - self.positions[j];
                let dist2 = delta.length_squared().max(EPSILON);
                let push = delta / dist2.sqrt() * (REPULSION / dist2);
                forces[i] += push;
                forces[j] -= push;
            }
        }

        // Hooke spring attraction along the level edges.
        for &(a, b) in &self.edges {
            let (a, b) = (a as usize, b as usize);
            if a >= n || b >= n {
                continue;
            }
            let delta = self.positions[b] - self.positions[a];
            let dist = delta.length().max(EPSILON);
            let pull = delta / dist * (SPRING_K * (dist - SPRING_LEN));
            forces[a] += pull;
            forces[b] -= pull;
        }

        // Integrate with damping; clamp the per-step displacement.
        let mut energy = 0.0_f32;
        for i in 0..n {
            // Weak gravity toward the origin keeps the cloud bounded.
            forces[i] -= self.positions[i] * GRAVITY;
            let v = ((self.velocities[i] + forces[i]) * DAMPING).clamp_length_max(MAX_SPEED);
            self.velocities[i] = v;
            self.positions[i] += v;
            energy += v.length_squared();
        }

        self.iterations += 1;
        if self.iterations >= MAX_ITERATIONS || energy / n as f32 <= SETTLE_ENERGY {
            self.settled = true;
        }
    }
}

/// Deterministic Fibonacci-sphere seeding — an even, rng-free spread
/// over a sphere whose radius grows with the cube root of the node
/// count so denser levels start more spacious.
fn seed_sphere(n: usize) -> Vec<Vec3> {
    let radius = SPRING_LEN * (n as f32).cbrt().max(1.0);
    let golden = std::f32::consts::PI * (3.0 - 5.0_f32.sqrt());
    (0..n)
        .map(|i| {
            let t = if n > 1 {
                i as f32 / (n - 1) as f32
            } else {
                0.5
            };
            let y = 1.0 - t * 2.0;
            let ring = (1.0 - y * y).max(0.0).sqrt();
            let theta = golden * i as f32;
            Vec3::new(theta.cos() * ring, y, theta.sin() * ring) * radius
        })
        .collect()
}

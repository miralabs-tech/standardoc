/// Per-frame pan speed in screen pixels when a Q/D/A/E key is held.
/// Tuned so a 1-second hold pans ~half a viewport at the default
/// camera distance; the host can override the binding map but the
/// speed stays a hardcoded internal constant (the magic number is a
/// taste call, not a configuration surface).
export const KEYBOARD_PAN_SPEED = 8;
/// Per-frame orbit speed in screen pixels when an arrow key is held.
/// Matches `Camera3D::ORBIT_SPEED` × this value ≈ 1.4° / frame at 60Hz.
export const KEYBOARD_ORBIT_SPEED = 3;
/// Per-frame multiplicative dolly factor when Z (forward) is held —
/// the inverse `1 / factor` applies for S (backward). At 60Hz this
/// gives ~150% zoom-in per second held.
export const KEYBOARD_DOLLY_FACTOR = 1.015;

/// Default key bindings — `event.code` strings so each action tracks
/// the PHYSICAL key position regardless of QWERTY / AZERTY layout.
/// The mapping below is the AZERTY-natural ZQSD + AE layout the user
/// asked for (and the QWERTY-equivalent WASD + QE):
///   • forward    Z (AZERTY) / W (QWERTY)  → `KeyW`
///   • backward   S                        → `KeyS`
///   • strafeL    D                        → `KeyD`
///   • strafeR    Q (AZERTY) / A (QWERTY)  → `KeyA`
///   • riseUp     A (AZERTY) / Q (QWERTY)  → `KeyQ`
///   • fallDown   E                        → `KeyE`
///   • orbit      ↑ ↓ ← →                   → arrows
///
/// Strafe is deliberately swapped vs the physical ZQSD/WASD diamond
/// (left=D, right=Q/A) — the user reported the default strafe read
/// reversed on screen for this camera, so the `event.code`s are
/// crossed here. Hosts can replace any subset via `keyBindings`.
export const DEFAULT_KEY_BINDINGS = {
  forward: ['KeyW'],
  backward: ['KeyS'],
  strafeLeft: ['KeyD'],
  strafeRight: ['KeyA'],
  riseUp: ['KeyQ'],
  fallDown: ['KeyE'],
  orbitUp: ['ArrowUp'],
  orbitDown: ['ArrowDown'],
  orbitLeft: ['ArrowLeft'],
  orbitRight: ['ArrowRight'],
  /// Recenter / refit the camera on the current scope — same effect
  /// as the Home button in the gizmo. Useful when the user has
  /// dollied / panned far from the scene and wants to snap back.
  reset: ['KeyR'],
} as const;

export type OverviewKeyAction = keyof typeof DEFAULT_KEY_BINDINGS;
export type OverviewKeyBindings = Readonly<Record<OverviewKeyAction, ReadonlyArray<string>>>;

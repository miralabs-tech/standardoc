export const SVG_NS = 'http://www.w3.org/2000/svg';

/// Orbit-ball world axes. Each entry is the unit world-vector being
/// projected onto the gizmo plus its render hints. The order matches
/// the SVG paint order: lines first, then tip dots, then labels — so
/// the labels always sit on top regardless of axis depth.
export const ORBIT_BALL_AXES: ReadonlyArray<{
  readonly id: string;
  readonly axis: readonly [number, number, number];
  readonly color: string;
  readonly letter: string;
}> = [
  { id: 'x', axis: [1, 0, 0], color: '#f48771', letter: 'X' }, // --sd-status-err
  { id: 'y', axis: [0, 1, 0], color: '#89d185', letter: 'Y' }, // --sd-status-ok
  { id: 'z', axis: [0, 0, 1], color: '#3794ff', letter: 'Z' }, // --sd-accent
];

export const ORBIT_BALL_SIZE = 76;
export const ORBIT_BALL_RADIUS = 28;
export const ORBIT_BALL_CENTER = ORBIT_BALL_SIZE / 2;

export interface OrbitAxisNodes {
  readonly id: string;
  readonly line: SVGLineElement;
  readonly tip: SVGCircleElement;
  readonly label: SVGTextElement;
}

export function projectOrbitAxis(
  axis: readonly [number, number, number],
  yaw: number,
  pitch: number,
): { sx: number; sy: number; depth: number } {
  const cp = Math.cos(pitch);
  const sp = Math.sin(pitch);
  const sy = Math.sin(yaw);
  const cy = Math.cos(yaw);
  // Camera "forward" = -(eye - target).normalize() = -dir.
  const fx = -cp * sy;
  const fy = -sp;
  const fz = -cp * cy;
  // right = forward × (0,-1,0)
  const rxr = fz;
  const ryr = 0;
  const rzr = -fx;
  const rl = Math.hypot(rxr, ryr, rzr) || 1;
  const rx = rxr / rl;
  const ry = ryr / rl;
  const rz = rzr / rl;
  // up_cam = right × forward
  const uxr = ry * fz - rz * fy;
  const uyr = rz * fx - rx * fz;
  const uzr = rx * fy - ry * fx;
  const ul = Math.hypot(uxr, uyr, uzr) || 1;
  const ux = uxr / ul;
  const uy = uyr / ul;
  const uz = uzr / ul;
  const dR = axis[0] * rx + axis[1] * ry + axis[2] * rz;
  const dU = axis[0] * ux + axis[1] * uy + axis[2] * uz;
  const dF = axis[0] * fx + axis[1] * fy + axis[2] * fz;
  // SVG y+ is down; up_cam projects to screen-up which is y-, so flip.
  return { sx: dR, sy: -dU, depth: dF };
}

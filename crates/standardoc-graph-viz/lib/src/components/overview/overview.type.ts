// Duck-typed subset of the wasm-bindgen `OverviewCanvas` JS surface
// that `<standardoc-overview>` touches. Keeps the lib decoupled from
// a specific WASM build — the host wires its own factory.

export interface OverviewCanvasFacade {
  tick(): void;
  invalidate(): void;
  resize(width: number, height: number): void;
  set_device_pixel_ratio(dpr: number): void;
  set_payload(json: string): void;
  set_on_cluster_click(cb: (cluster_id: number) => void): void;
  set_on_cluster_hover(cb: (cluster_id: number | null) => void): void;
  on_pointer_move(x: number, y: number): void;
  /**
   * `panMode` toggles drag semantics: `false` (plain left-drag)
   * orbits around the target; `true` (alt + left-drag, Maya / Figma
   * convention) pans the target along the camera plane.
   */
  on_pointer_down(x: number, y: number, button: number, panMode: boolean): void;
  on_pointer_up(x: number, y: number, button: number): void;
  on_pointer_leave(): void;
  on_wheel(x: number, y: number, deltaY: number): void;
  fit(): void;
  set_camera_preset(preset: string): void;
  /**
   * Drive the camera orbit directly (used by the orbit-ball widget
   * in `<standardoc-overview>`). `dx`/`dy` are screen-pixel deltas
   * — same scaling as the in-canvas drag path.
   */
  orbit_camera(dx: number, dy: number): void;
  /**
   * Drive the camera pan directly (used by Q/D strafe and A/E
   * rise-fall keyboard nav in `<standardoc-overview>`). `dx`/`dy`
   * follow the same grab semantics as the alt-drag canvas path.
   */
  pan_camera(dx: number, dy: number): void;
  /**
   * Dolly the camera along its forward axis (used by Z/S keyboard
   * nav). `factor > 1` moves forward (closer to target); `factor < 1`
   * moves backward.
   */
  dolly_camera(factor: number): void;
  /**
   * Cap on the number of cluster text labels rendered each frame. `0`
   * disables the cap (all labels render); any other value picks the
   * N closest clusters to the camera. Halo + dot still render so the
   * topology stays readable when the cap is tight.
   */
  set_max_visible_labels(n: number): void;
  /**
   * Toggle inter-module ("cross") edge rendering. Parent-child
   * structural edges (the FQDN spine) always render; cross edges
   * are off by default to keep the depth-stacked layout readable
   * and surface them on demand.
   */
  set_show_cross_edges(show: boolean): void;
  /**
   * Cap label rendering to nodes whose FQDN depth is `<= cap`. A
   * very large number (e.g. 2 ** 32 - 1) disables the cap; `0`
   * paints only root-package labels and leaves deeper nodes as
   * unlabelled halos. The host clamps this to `0` at workspace
   * scope so the high-level view stays readable across hundreds
   * of modules.
   */
  set_label_depth_cap(cap: number): void;
  readonly cluster_count: number;
  readonly edge_count: number;
  /** Current orbit yaw in radians — synced into the orbit-ball widget. */
  readonly camera_yaw: number;
  /** Current orbit pitch in radians — synced into the orbit-ball widget. */
  readonly camera_pitch: number;
}

export type OverviewCanvasFactory = (
  canvas: HTMLCanvasElement,
  width: number,
  height: number,
  dpr: number,
) => OverviewCanvasFacade | Promise<OverviewCanvasFacade>;

export interface OverviewReadyDetail {
  readonly canvas: OverviewCanvasFacade;
}

export interface OverviewClusterClickDetail {
  readonly clusterId: number;
}

export interface OverviewClusterHoverDetail {
  readonly clusterId: number | null;
}

export interface OverviewErrorDetail {
  readonly source: 'canvas-init' | 'set-payload';
  readonly message: string;
}

/**
 * Describes the current Overview scope label shown in the breadcrumb
 * overlay. `null` (workspace mode) hides the overlay entirely; any
 * other value renders the breadcrumb `← Workspace › <label>`.
 */
export type OverviewScopeLabel = string | null;

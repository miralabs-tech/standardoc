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
  on_pointer_down(x: number, y: number, button: number): void;
  on_pointer_up(x: number, y: number, button: number): void;
  on_pointer_leave(): void;
  on_wheel(x: number, y: number, deltaY: number): void;
  fit(): void;
  set_camera_preset(preset: string): void;
  /**
   * Cap on the number of cluster text labels rendered each frame. `0`
   * disables the cap (all labels render); any other value picks the
   * N closest clusters to the camera. Halo + dot still render so the
   * topology stays readable when the cap is tight.
   */
  set_max_visible_labels(n: number): void;
  readonly cluster_count: number;
  readonly edge_count: number;
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

// Duck-typed subset of the wasm-bindgen `FocusGraphCanvas` JS surface
// that `<standardoc-focus-graph>` touches.

export interface FocusGraphCanvasFacade {
  tick(): void;
  invalidate(): void;
  resize(width: number, height: number): void;
  set_device_pixel_ratio(dpr: number): void;
  set_payload(json: string): void;
  set_hop_count(hops: number): void;
  set_on_node_click(cb: (fqdn: string) => void): void;
  set_on_node_hover(cb: (fqdn: string | null) => void): void;
  /**
   * Fired when the user clicks a "+N more" overflow badge. The canvas
   * has already raised the per-bucket cap and re-laid by the time this
   * fires — the callback is informational so hosts can mirror the
   * expansion in a side drawer or log analytics.
   */
  set_on_overflow_click(cb: (bucket: string, hiddenCount: number, newCap: number) => void): void;
  on_pointer_move(x: number, y: number): void;
  on_pointer_down(x: number, y: number, button: number): void;
  on_pointer_up(x: number, y: number, button: number): void;
  on_pointer_leave(): void;
  on_wheel(x: number, y: number, deltaY: number): void;
  fit(): void;
  label_layout(): string;
  readonly node_count: number;
  readonly edge_count: number;
  readonly current_hop_count: number;
  /**
   * FQDN of the currently focal symbol — empty string when no
   * payload has been pushed yet. `<standardoc-focus-graph>` reads
   * this each rAF tick to keep the breadcrumb in sync without
   * needing the host to mirror its focus state.
   */
  readonly focus_fqdn: string;
}

export type FocusGraphCanvasFactory = (
  canvas: HTMLCanvasElement,
  width: number,
  height: number,
  dpr: number,
) => FocusGraphCanvasFacade | Promise<FocusGraphCanvasFacade>;

export interface FocusGraphReadyDetail {
  readonly canvas: FocusGraphCanvasFacade;
}

export interface FocusGraphNodeClickDetail {
  readonly fqdn: string;
}

export interface FocusGraphNodeHoverDetail {
  readonly fqdn: string | null;
}

export interface FocusGraphHopChangeDetail {
  readonly hops: number;
}

/// Emitted when the user clicks a "+N more" overflow badge on a Focus
/// graph bucket. `bucket` is the snake_case bucket name (used_by,
/// uses_types, calls, imports, imported_by, tested_by,
/// implements_extends, indirect). `hiddenCount` is the count surfaced
/// in the badge before the click; `newCap` is the bucket cap after the
/// in-canvas expansion (canvas already re-laid by the time this fires).
export interface FocusBucketExpandDetail {
  readonly bucket: string;
  readonly hiddenCount: number;
  readonly newCap: number;
}

export interface FocusGraphErrorDetail {
  readonly source: 'canvas-init' | 'set-payload';
  readonly message: string;
}

/// Emitted when the user clicks the back arrow in the focus breadcrumb.
/// The host decides what "back" means (pop history, return to overview,
/// navigate to parent module) — the element only signals the intent.
export interface FocusGraphBackDetail {
  /// FQDN that was focal at the time of the back click. The host can
  /// use this to record history or to compute "parent" relative to it.
  readonly from: string;
}

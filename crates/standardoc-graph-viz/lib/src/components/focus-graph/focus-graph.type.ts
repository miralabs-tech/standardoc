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

export interface FocusGraphErrorDetail {
  readonly source: 'canvas-init' | 'set-payload';
  readonly message: string;
}

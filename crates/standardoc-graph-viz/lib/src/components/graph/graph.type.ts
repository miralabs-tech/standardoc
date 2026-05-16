import type { RenderMode } from '../../types';

/// Duck-typed subset of the wasm-bindgen `GraphEngine` JS surface that
/// `<standardoc-graph>` touches. The lib stays decoupled from a
/// specific WASM build — the host wires its own factory.
export interface GraphEngineFacade {
	tick(): void;
	last_tick_us(): number;
	symbol_count(): number;
	edge_count(): number;
	mode(): string;
	gpu_active(): boolean;
	gpu_instance_count(): number;
	gpu_instance_capacity(): number;

	load_graph(json: string): void;
	set_edges(json: string): void;
	set_palette(json: string): void;
	fit(): void;
	reset_zoom(): void;
	resize(width: number, height: number): void;
	set_device_pixel_ratio(dpr: number): void;
	invalidate(): void;

	on_pointer_move(x: number, y: number): void;
	on_pointer_down(x: number, y: number, button: number): void;
	on_pointer_up(x: number, y: number, button: number): void;
	on_pointer_leave(): void;
	on_wheel(x: number, y: number, deltaY: number): void;

	set_mode(mode: string): void;
	enable_webgpu(canvas: HTMLCanvasElement): Promise<void>;

	set_on_node_hover(cb: (fqdn: string | null) => void): void;
	set_on_node_click(cb: (fqdn: string) => void): void;
}

/// Engine factory the host provides. Called once by the component
/// after the canvases are in the DOM. Sync OR async — `Promise` is
/// returned by wasm-bindgen constructors that defer to JS imports.
export type GraphEngineFactory = (
	canvas: HTMLCanvasElement,
	width: number,
	height: number,
	dpr: number,
) => GraphEngineFacade | Promise<GraphEngineFacade>;

export interface GraphReadyDetail {
	readonly engine: GraphEngineFacade;
}

export interface GraphHoverDetail {
	readonly fqdn: string | null;
}

export interface GraphClickDetail {
	readonly fqdn: string;
}

export interface GraphModeChangeDetail {
	readonly mode: RenderMode;
}

export type GraphErrorSource = 'engine-init' | 'webgpu-init' | 'set-mode';

export interface GraphErrorDetail {
	readonly source: GraphErrorSource;
	readonly message: string;
}

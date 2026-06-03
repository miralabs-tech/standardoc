// No-op WASM stub for the shell harness. The shell drives the canvas
// through the duck-typed facades (OverviewCanvasFacade /
// FocusGraphCanvasFacade); the real WebGPU rendering is out of scope
// for shell wiring tests. These stubs satisfy every method the
// component elements + mountShell touch, returning inert values, so
// `sd-overview-ready` / `sd-focus-graph-ready` fire without a GPU.

import type { ShellWasm } from '../../src/shell/mount';
import type {
  FocusGraphCanvasFacade,
  OverviewCanvasFacade,
} from '../../src/index';

class StubOverviewCanvas implements OverviewCanvasFacade {
  tick(): void {}
  invalidate(): void {}
  resize(): void {}
  set_device_pixel_ratio(): void {}
  set_payload(): void {}
  set_on_cluster_click(): void {}
  set_on_cluster_hover(): void {}
  on_pointer_move(): void {}
  on_pointer_down(): void {}
  on_pointer_up(): void {}
  on_pointer_leave(): void {}
  on_wheel(): void {}
  fit(): void {}
  set_camera_preset(): void {}
  orbit_camera(): void {}
  pan_camera(): void {}
  dolly_camera(): void {}
  set_max_visible_labels(): void {}
  set_show_cross_edges(): void {}
  set_label_depth_cap(): void {}
  readonly cluster_count = 0;
  readonly edge_count = 0;
  readonly camera_yaw = 0;
  readonly camera_pitch = 0;
}

class StubFocusGraphCanvas implements FocusGraphCanvasFacade {
  tick(): void {}
  invalidate(): void {}
  resize(): void {}
  set_device_pixel_ratio(): void {}
  set_payload(): void {}
  set_hop_count(): void {}
  set_on_node_click(): void {}
  set_on_node_hover(): void {}
  set_on_overflow_click(): void {}
  on_pointer_move(): void {}
  on_pointer_down(): void {}
  on_pointer_up(): void {}
  on_pointer_leave(): void {}
  on_wheel(): void {}
  fit(): void {}
  label_layout(): string {
    return '[]';
  }
  readonly node_count = 0;
  readonly edge_count = 0;
  readonly current_hop_count = 1;
  readonly focus_fqdn = '';
}

export const stubWasm: ShellWasm = {
  init: () => Promise.resolve(),
  OverviewCanvas: StubOverviewCanvas as unknown as ShellWasm['OverviewCanvas'],
  FocusGraphCanvas: StubFocusGraphCanvas as unknown as ShellWasm['FocusGraphCanvas'],
};

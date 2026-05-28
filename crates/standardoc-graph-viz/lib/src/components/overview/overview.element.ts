/**
 * `<standardoc-overview>` — wraps the wasm-bindgen `OverviewCanvas`
 * for the workspace-level nebula view. Mirrors the pattern of
 * `<standardoc-graph>` but driven by the new Phase 3 cluster/edge
 * payload shape rather than the legacy GraphEngine API.
 *
 * Owns:
 *   - the canvas element
 *   - pointer/wheel/resize events with CSS-pixel coords
 *   - rAF loop while the engine is mounted
 *   - scope breadcrumb DOM overlay (top-left pill, host-driven label)
 *
 * Does NOT own:
 *   - WASM init (host provides `canvasFactory`)
 *   - cluster/edge data (host calls `el.canvas.set_payload(json)`
 *     via the `.canvas` getter once `sd-overview-ready` fires)
 *   - scope policy (host owns workspace/project/folder state, just
 *     pushes the label here via `scopeLabel`)
 *
 * Events emitted:
 *   - `sd-overview-ready`         detail: { canvas }
 *   - `sd-overview-cluster-hover` detail: { clusterId | null }
 *   - `sd-overview-cluster-click` detail: { clusterId }
 *   - `sd-overview-back`          detail: {}  — breadcrumb back click
 *   - `sd-overview-error`         detail: { source, message }
 */

import classigo from 'classigo';

import { STANDARDOC_SELECT_TAG, type SelectChangeDetail, type SelectElement } from '../select';
import type {
  OverviewCanvasFacade,
  OverviewCanvasFactory,
  OverviewClusterClickDetail,
  OverviewClusterHoverDetail,
  OverviewErrorDetail,
  OverviewReadyDetail,
  OverviewScopeLabel,
} from './overview.type';
import {
  C,
  DEFAULT_LABEL_LIMIT,
  DEFAULT_PRESET,
  LABEL_LIMIT_OPTIONS,
  PRESET_BUTTONS,
  STANDARDOC_OVERVIEW_TAG,
} from './overview.constants';
import {
  CROSS_EDGE_KINDS,
  readPersistedCrossEdges,
  writePersistedCrossEdges,
} from './overview.cross-edges';
import {
  ORBIT_BALL_AXES,
  ORBIT_BALL_CENTER,
  ORBIT_BALL_RADIUS,
  ORBIT_BALL_SIZE,
  type OrbitAxisNodes,
  SVG_NS,
  projectOrbitAxis,
} from './overview.orbit-ball';
import {
  DEFAULT_KEY_BINDINGS,
  KEYBOARD_DOLLY_FACTOR,
  KEYBOARD_ORBIT_SPEED,
  KEYBOARD_PAN_SPEED,
  type OverviewKeyBindings,
} from './overview.key-bindings';

export { STANDARDOC_OVERVIEW_TAG };

export class OverviewElement extends HTMLElement {
  #mounted = false;
  #initStarted = false;
  #canvas: OverviewCanvasFacade | null = null;
  #factory: OverviewCanvasFactory | null = null;
  #observer: ResizeObserver | null = null;
  #rafHandle: number | null = null;
  #scopeLabel: OverviewScopeLabel = null;
  #nodes: {
    root: HTMLElement;
    canvas: HTMLCanvasElement;
    breadcrumb: HTMLElement;
    gizmo: HTMLElement;
    ball: SVGSVGElement;
    ballAxes: ReadonlyArray<OrbitAxisNodes>;
    presetBtns: ReadonlyArray<HTMLButtonElement>;
    labelSelect: SelectElement;
    crossEdgesBtn: HTMLButtonElement;
    legend: HTMLElement;
  } | null = null;
  #activePreset = DEFAULT_PRESET;
  #labelLimit = DEFAULT_LABEL_LIMIT;
  #showCrossEdges = readPersistedCrossEdges();
  #crossEdgeKinds: ReadonlyArray<string> = [];
  /// Currently-held key codes (`event.code`), populated by the
  /// keyboard nav listeners. The set is read in `#loop` so multiple
  /// keys pressed at once compose smoothly (e.g. Z+D for diagonal).
  #pressedKeys = new Set<string>();
  #keyBindings: OverviewKeyBindings = DEFAULT_KEY_BINDINGS;

  set canvasFactory(factory: OverviewCanvasFactory) {
    this.#factory = factory;
    this.#tryInit();
  }

  get canvas(): OverviewCanvasFacade | null {
    return this.#canvas;
  }

  /**
   * Host-pushed scope label. `null` hides the breadcrumb overlay
   * (workspace mode). Any other value renders `← Workspace › <label>`
   * top-left; clicking the back arrow emits `sd-overview-back`.
   */
  set scopeLabel(label: OverviewScopeLabel) {
    if (label === this.#scopeLabel) return;
    this.#scopeLabel = label;
    this.#renderBreadcrumb();
  }

  get scopeLabel(): OverviewScopeLabel {
    return this.#scopeLabel;
  }

  /// Override the default ZQSD / arrow-key bindings. Each action maps
  /// to a list of `KeyboardEvent.code` strings; binding zero codes
  /// disables the action. Setting partial bindings merges with the
  /// default so hosts can rebind a single action without re-declaring
  /// the full set.
  set keyBindings(partial: Partial<OverviewKeyBindings>) {
    this.#keyBindings = { ...DEFAULT_KEY_BINDINGS, ...partial };
  }

  get keyBindings(): OverviewKeyBindings {
    return this.#keyBindings;
  }

  connectedCallback(): void {
    if (this.#mounted) return;
    this.#mounted = true;
    this.#render();
    this.#tryInit();
  }

  disconnectedCallback(): void {
    if (this.#rafHandle !== null) {
      cancelAnimationFrame(this.#rafHandle);
      this.#rafHandle = null;
    }
    this.#observer?.disconnect();
    this.#observer = null;
  }

  #render(): void {
    const root = document.createElement('div');
    root.className = C.overview;
    const canvas = document.createElement('canvas');
    canvas.className = C.canvas;
    root.appendChild(canvas);
    const breadcrumb = document.createElement('div');
    breadcrumb.className = C.breadcrumb;
    breadcrumb.style.display = 'none';
    root.appendChild(breadcrumb);
    const gizmo = document.createElement('div');
    gizmo.className = C.gizmo;

    // Orbit-ball widget — Maya/Blender-style 3D orientation indicator
    // floating in the top-right of the panel. Purely decorative: the
    // axes track the current yaw + pitch so the user reads the world
    // frame at a glance, but the actual camera nav lives elsewhere
    // (canvas drag for orbit, alt-drag for pan, ZQSD / arrows for
    // keyboard nav).
    const ball = document.createElementNS(SVG_NS, 'svg');
    ball.classList.add(C.ball);
    ball.setAttribute('viewBox', `0 0 ${ORBIT_BALL_SIZE} ${ORBIT_BALL_SIZE}`);
    ball.setAttribute('width', String(ORBIT_BALL_SIZE));
    ball.setAttribute('height', String(ORBIT_BALL_SIZE));
    ball.setAttribute('role', 'img');
    ball.setAttribute('aria-label', 'Camera orientation');
    // Sphere outline (front-most circle) + 2 ellipse "wireframe" rings
    // for visual depth cue. All static — no per-frame update needed.
    const outline = document.createElementNS(SVG_NS, 'circle');
    outline.setAttribute('cx', String(ORBIT_BALL_CENTER));
    outline.setAttribute('cy', String(ORBIT_BALL_CENTER));
    outline.setAttribute('r', String(ORBIT_BALL_RADIUS));
    outline.setAttribute('fill', 'rgba(120, 140, 180, 0.06)');
    outline.setAttribute('stroke', 'rgba(180, 200, 230, 0.20)');
    outline.setAttribute('stroke-width', '1');
    ball.appendChild(outline);
    const ringH = document.createElementNS(SVG_NS, 'ellipse');
    ringH.setAttribute('cx', String(ORBIT_BALL_CENTER));
    ringH.setAttribute('cy', String(ORBIT_BALL_CENTER));
    ringH.setAttribute('rx', String(ORBIT_BALL_RADIUS));
    ringH.setAttribute('ry', '6');
    ringH.setAttribute('fill', 'none');
    ringH.setAttribute('stroke', 'rgba(180, 200, 230, 0.10)');
    ringH.setAttribute('stroke-width', '1');
    ball.appendChild(ringH);
    const ringV = document.createElementNS(SVG_NS, 'ellipse');
    ringV.setAttribute('cx', String(ORBIT_BALL_CENTER));
    ringV.setAttribute('cy', String(ORBIT_BALL_CENTER));
    ringV.setAttribute('rx', '6');
    ringV.setAttribute('ry', String(ORBIT_BALL_RADIUS));
    ringV.setAttribute('fill', 'none');
    ringV.setAttribute('stroke', 'rgba(180, 200, 230, 0.10)');
    ringV.setAttribute('stroke-width', '1');
    ball.appendChild(ringV);
    // Axis lines (front then back determined per-tick via opacity).
    const ballAxes: OrbitAxisNodes[] = ORBIT_BALL_AXES.map(spec => {
      const line = document.createElementNS(SVG_NS, 'line');
      line.classList.add(C.ballAxis);
      line.dataset['axis'] = spec.id;
      line.setAttribute('x1', String(ORBIT_BALL_CENTER));
      line.setAttribute('y1', String(ORBIT_BALL_CENTER));
      line.setAttribute('x2', String(ORBIT_BALL_CENTER));
      line.setAttribute('y2', String(ORBIT_BALL_CENTER));
      line.setAttribute('stroke', spec.color);
      line.setAttribute('stroke-width', '2');
      line.setAttribute('stroke-linecap', 'round');
      ball.appendChild(line);
      const tip = document.createElementNS(SVG_NS, 'circle');
      tip.classList.add(C.ballTip);
      tip.dataset['axis'] = spec.id;
      tip.setAttribute('cx', String(ORBIT_BALL_CENTER));
      tip.setAttribute('cy', String(ORBIT_BALL_CENTER));
      tip.setAttribute('r', '7');
      tip.setAttribute('fill', spec.color);
      ball.appendChild(tip);
      const label = document.createElementNS(SVG_NS, 'text');
      label.classList.add(C.ballLabel);
      label.dataset['axis'] = spec.id;
      label.setAttribute('x', String(ORBIT_BALL_CENTER));
      label.setAttribute('y', String(ORBIT_BALL_CENTER));
      label.setAttribute('text-anchor', 'middle');
      label.setAttribute('dominant-baseline', 'central');
      label.setAttribute('font-size', '9');
      label.setAttribute('font-weight', '700');
      label.setAttribute('fill', '#0b0e14');
      label.textContent = spec.letter;
      ball.appendChild(label);
      return { id: spec.id, line, tip, label };
    });
    // Ball floats top-right of the overview panel as its own widget —
    // separate from the gizmo so the user reads it as an orientation
    // indicator, not a control.
    root.appendChild(ball);

    // Compact preset row in the gizmo — home + 4 preset chips. Camera
    // nav lives elsewhere (canvas drag, alt-drag pan, ZQSD / arrow
    // keys), so the presets are just one-click snaps.
    const presetRow = document.createElement('div');
    presetRow.className = C.gizmoRow;
    const home = document.createElement('button');
    home.type = 'button';
    home.className = C.gizmoBtn;
    home.title = 'Re-fit camera to scope';
    home.textContent = '⌂';
    home.addEventListener('click', () => { this.#canvas?.fit(); });
    presetRow.appendChild(home);
    const presetGroup = document.createElement('div');
    presetGroup.className = C.gizmoGroup;
    const presetBtns: HTMLButtonElement[] = [];
    for (const p of PRESET_BUTTONS) {
      const btn = document.createElement('button');
      btn.type = 'button';
      btn.className = C.gizmoBtn;
      btn.dataset['preset'] = p.preset;
      btn.title = p.title;
      btn.textContent = p.label;
      btn.addEventListener('click', () => {
        this.#activePreset = p.preset;
        this.#canvas?.set_camera_preset(p.preset);
        this.#syncGizmo();
      });
      presetGroup.appendChild(btn);
      presetBtns.push(btn);
    }
    presetRow.appendChild(presetGroup);
    gizmo.appendChild(presetRow);

    // Label-cap row — themed <standardoc-select> so the open popover
    // honours `--sd-*` tokens (native <select> ignores them).
    const labelRow = document.createElement('div');
    labelRow.className = C.gizmoRow;
    const labelHint = document.createElement('span');
    labelHint.className = C.gizmoLabel;
    labelHint.textContent = 'labels';
    const labelSelect = document.createElement(STANDARDOC_SELECT_TAG);
    labelSelect.className = C.gizmoSelect;
    labelSelect.title = 'Max cluster labels rendered (by camera distance)';
    labelSelect.options = LABEL_LIMIT_OPTIONS;
    labelSelect.value = this.#labelLimit;
    labelSelect.placement = 'top';
    labelSelect.addEventListener('sd-select-change', e => {
      const detail = (e as CustomEvent<SelectChangeDetail>).detail;
      const raw = detail.value;
      const n = typeof raw === 'number' ? raw : Number.parseInt(String(raw), 10);
      this.#labelLimit = Number.isFinite(n) ? n : 0;
      this.#canvas?.set_max_visible_labels(this.#labelLimit);
    });
    labelRow.append(labelHint, labelSelect);
    gizmo.appendChild(labelRow);

    // Cross-edges toggle — when off (default), only the FQDN spine
    // (parent_child edges) renders. Flip on to surface inter-module
    // imports/calls/uses-type as glow strands across the depth planes.
    const crossRow = document.createElement('div');
    crossRow.className = C.gizmoRow;
    const crossHint = document.createElement('span');
    crossHint.className = C.gizmoLabel;
    crossHint.textContent = 'cross';
    const crossEdgesBtn = document.createElement('button');
    crossEdgesBtn.type = 'button';
    crossEdgesBtn.className = C.gizmoBtn;
    crossEdgesBtn.title = 'Toggle inter-module edges (imports / calls / uses-type)';
    crossEdgesBtn.textContent = this.#showCrossEdges ? 'on' : 'off';
    crossEdgesBtn.addEventListener('click', () => {
      this.#showCrossEdges = !this.#showCrossEdges;
      writePersistedCrossEdges(this.#showCrossEdges);
      this.#canvas?.set_show_cross_edges(this.#showCrossEdges);
      this.#syncCrossEdgesBtn();
      this.#renderLegend();
    });
    crossRow.append(crossHint, crossEdgesBtn);
    gizmo.appendChild(crossRow);

    root.appendChild(gizmo);

    // Mini-legend pinned bottom-left — one chip per IR edge kind
    // present in the current scope's cross edges. Hidden whenever
    // the cross-edge toggle is off (the spine alone is monochrome
    // by design — adding a legend then would be lying).
    const legend = document.createElement('div');
    legend.className = classigo(C.legend, C.legendEmpty);
    root.appendChild(legend);

    // Focusable so the ZQSD / arrow-key nav listeners can pick up
    // keydown events scoped to this panel. `outline: none` in SCSS
    // hides the default browser outline; `:focus-visible` paints a
    // subtle accent border instead.
    root.tabIndex = 0;
    this.replaceChildren(root);
    this.#nodes = { root, canvas, breadcrumb, gizmo, ball, ballAxes, presetBtns, labelSelect, crossEdgesBtn, legend };
    this.#wirePointer();
    this.#wireKeyboard();
    this.#renderBreadcrumb();
    this.#syncGizmo();
    this.#syncCrossEdgesBtn();
    this.#renderLegend();
    this.#syncOrbitBall();
  }

  #syncGizmo(): void {
    const n = this.#nodes;
    if (n === null) return;
    for (const btn of n.presetBtns) {
      const isActive = btn.dataset['preset'] === this.#activePreset;
      btn.className = classigo(C.gizmoBtn, isActive && C.gizmoBtnActive);
    }
  }

  #syncCrossEdgesBtn(): void {
    const n = this.#nodes;
    if (n === null) return;
    n.crossEdgesBtn.textContent = this.#showCrossEdges ? 'on' : 'off';
    n.crossEdgesBtn.className = classigo(C.gizmoBtn, this.#showCrossEdges && C.gizmoBtnActive);
  }

  /**
   * Host-pushed list of IR edge kinds present in the current scope's
   * cross edges (`CALLS`, `IMPORTS`, `USES_TYPE`, …). The legend
   * renders one chip per known kind; unknown kinds are skipped so the
   * chip row stays in sync with the canvas palette.
   */
  set crossEdgeKinds(kinds: ReadonlyArray<string>) {
    this.#crossEdgeKinds = kinds;
    this.#renderLegend();
  }

  get crossEdgeKinds(): ReadonlyArray<string> {
    return this.#crossEdgeKinds;
  }

  #renderLegend(): void {
    const n = this.#nodes;
    if (n === null) return;
    // Hide the legend whenever cross-edges are off — the spine alone
    // is monochrome and labelling it would just add noise. Also hide
    // when the scope has no cross kinds at all (e.g. drilled deep
    // enough that no inter-module edges remain).
    const visible = this.#showCrossEdges && this.#crossEdgeKinds.length > 0;
    if (!visible) {
      n.legend.className = classigo(C.legend, C.legendEmpty);
      n.legend.replaceChildren();
      return;
    }
    const present = new Set(this.#crossEdgeKinds);
    const chips = CROSS_EDGE_KINDS.filter(spec => present.has(spec.kind));
    const frag = document.createDocumentFragment();
    for (const chip of chips) {
      const item = document.createElement('span');
      item.className = C.legendItem;
      item.style.color = chip.color;
      const swatch = document.createElement('span');
      swatch.className = chip.dashed
        ? classigo(C.legendSwatch, C.legendSwatchDashed)
        : C.legendSwatch;
      const label = document.createElement('span');
      label.textContent = chip.label;
      label.style.color = 'var(--sd-fg, #cccccc)';
      item.append(swatch, label);
      frag.appendChild(item);
    }
    n.legend.className = C.legend;
    n.legend.replaceChildren(frag);
  }

  #wireKeyboard(): void {
    const n = this.#nodes;
    if (n === null) return;
    // ZQSD on AZERTY = WASD on QWERTY = `event.code` KeyW/A/S/D in both.
    // We listen on the panel root so the nav only fires when the user
    // has clicked into the overview (focus state). Browser-default
    // arrow-key scrolling is suppressed via `preventDefault`.
    n.root.addEventListener('keydown', e => {
      if (this.#isNavCode(e.code)) {
        e.preventDefault();
        // Reset is a one-shot action — fire on keydown directly,
        // don't add to the pressed-keys set (no per-frame loop).
        if (this.#keyBindings.reset.includes(e.code)) {
          this.#canvas?.fit();
          return;
        }
        this.#pressedKeys.add(e.code);
      }
    });
    n.root.addEventListener('keyup', e => {
      this.#pressedKeys.delete(e.code);
    });
    // Blur clears the pressed-key set so a key held while tabbing away
    // doesn't keep the camera drifting after focus returns. Window-
    // level blur catches the alt-tab case where the panel root's blur
    // wouldn't fire because focus moved to another window entirely.
    n.root.addEventListener('blur', () => { this.#pressedKeys.clear(); });
    window.addEventListener('blur', () => { this.#pressedKeys.clear(); });
    // Tab hidden = no rAF firing, but a key held when the tab gets
    // hidden would resume mutating the camera the instant the tab
    // returns. Clear on visibility loss so the user always lands on
    // a stationary camera when they switch back.
    document.addEventListener('visibilitychange', () => {
      if (document.visibilityState !== 'visible') this.#pressedKeys.clear();
    });
    // Focus the panel when the user clicks the canvas — without this
    // the first key press after panel mount does nothing because focus
    // is still on whatever the user clicked before.
    n.canvas.addEventListener('pointerdown', () => {
      n.root.focus({ preventScroll: true });
    });
  }

  #isNavCode(code: string): boolean {
    const b = this.#keyBindings;
    return (
      b.forward.includes(code) ||
      b.backward.includes(code) ||
      b.strafeLeft.includes(code) ||
      b.strafeRight.includes(code) ||
      b.riseUp.includes(code) ||
      b.fallDown.includes(code) ||
      b.orbitUp.includes(code) ||
      b.orbitDown.includes(code) ||
      b.orbitLeft.includes(code) ||
      b.orbitRight.includes(code) ||
      b.reset.includes(code)
    );
  }

  #applyKeyboardNav(): void {
    if (this.#canvas === null || this.#pressedKeys.size === 0) return;
    const b = this.#keyBindings;
    const has = (codes: ReadonlyArray<string>) => codes.some(c => this.#pressedKeys.has(c));
    // Strafe (Q/D) + rise-fall (A/E) → pan target along the camera
    // plane. Grab semantics (same as alt-drag): a positive `dx` shifts
    // the target left so the world drifts right under the viewport.
    // "Look this direction" intent inverts the sign vs the literal
    // screen-axis intuition.
    let panDx = 0;
    let panDy = 0;
    if (has(b.strafeLeft)) panDx += KEYBOARD_PAN_SPEED;
    if (has(b.strafeRight)) panDx -= KEYBOARD_PAN_SPEED;
    if (has(b.riseUp)) panDy += KEYBOARD_PAN_SPEED;
    if (has(b.fallDown)) panDy -= KEYBOARD_PAN_SPEED;
    if (panDx !== 0 || panDy !== 0) {
      this.#canvas.pan_camera(panDx, panDy);
    }
    // Forward / backward (Z/S) → dolly along the camera forward axis.
    // Multiplicative so the per-frame change feels consistent at any
    // current camera distance.
    let dollyFactor = 1;
    if (has(b.forward)) dollyFactor *= KEYBOARD_DOLLY_FACTOR;
    if (has(b.backward)) dollyFactor /= KEYBOARD_DOLLY_FACTOR;
    if (dollyFactor !== 1) {
      this.#canvas.dolly_camera(dollyFactor);
    }
    // Orbit (arrow keys) → yaw / pitch around target.
    let orbitDx = 0;
    let orbitDy = 0;
    if (has(b.orbitLeft)) orbitDx -= KEYBOARD_ORBIT_SPEED;
    if (has(b.orbitRight)) orbitDx += KEYBOARD_ORBIT_SPEED;
    if (has(b.orbitUp)) orbitDy -= KEYBOARD_ORBIT_SPEED;
    if (has(b.orbitDown)) orbitDy += KEYBOARD_ORBIT_SPEED;
    if (orbitDx !== 0 || orbitDy !== 0) {
      this.#canvas.orbit_camera(orbitDx, orbitDy);
      this.#activePreset = '';
      this.#syncGizmo();
    }
  }

  #syncOrbitBall(): void {
    const n = this.#nodes;
    if (n === null) return;
    // The first sync happens before the canvas is bound — fall back to
    // a default orientation so the ball reads as "neutral 3/4" instead
    // of an empty disc.
    const yaw = this.#canvas?.camera_yaw ?? 0.7;
    const pitch = this.#canvas?.camera_pitch ?? 0.5;
    for (const node of n.ballAxes) {
      const spec = ORBIT_BALL_AXES.find(a => a.id === node.id);
      if (spec === undefined) continue;
      const proj = projectOrbitAxis(spec.axis, yaw, pitch);
      const tipX = ORBIT_BALL_CENTER + proj.sx * ORBIT_BALL_RADIUS;
      const tipY = ORBIT_BALL_CENTER + proj.sy * ORBIT_BALL_RADIUS;
      node.line.setAttribute('x2', tipX.toFixed(2));
      node.line.setAttribute('y2', tipY.toFixed(2));
      node.tip.setAttribute('cx', tipX.toFixed(2));
      node.tip.setAttribute('cy', tipY.toFixed(2));
      node.label.setAttribute('x', tipX.toFixed(2));
      node.label.setAttribute('y', tipY.toFixed(2));
      // Behind = axis pointing away from camera. Fade so the front-most
      // axes read as the "live" ones. Solid in front (depth < 0).
      const behind = proj.depth > 0;
      node.line.classList.toggle(C.ballAxisBehind, behind);
      node.tip.classList.toggle(C.ballAxisBehind, behind);
      node.label.classList.toggle(C.ballAxisBehind, behind);
    }
  }

  #renderBreadcrumb(): void {
    const n = this.#nodes;
    if (n === null) return;
    if (this.#scopeLabel === null) {
      n.breadcrumb.style.display = 'none';
      n.breadcrumb.replaceChildren();
      return;
    }
    n.breadcrumb.style.display = 'flex';
    const back = document.createElement('button');
    back.type = 'button';
    back.className = C.breadcrumbBack;
    back.textContent = '← Workspace';
    back.title = 'Back to workspace';
    back.addEventListener('click', () => {
      this.dispatchEvent(new CustomEvent('sd-overview-back', {
        detail: {}, bubbles: true, composed: true,
      }));
    });
    const sep = document.createElement('span');
    sep.className = C.breadcrumbSep;
    sep.textContent = '›';
    const current = document.createElement('span');
    current.className = C.breadcrumbCurrent;
    current.textContent = this.#scopeLabel;
    n.breadcrumb.replaceChildren(back, sep, current);
  }

  #wirePointer(): void {
    const n = this.#nodes;
    if (n === null) return;
    n.canvas.addEventListener('pointermove', e => {
      if (this.#canvas === null) return;
      const { x, y } = this.#cssCoords(e);
      this.#canvas.on_pointer_move(x, y);
    });
    n.canvas.addEventListener('pointerdown', e => {
      if (this.#canvas === null) return;
      const { x, y } = this.#cssCoords(e);
      n.canvas.setPointerCapture(e.pointerId);
      n.root.className = classigo(C.overview, C.grabbing);
      // Alt-drag = pan (Maya / Figma convention), plain drag = orbit.
      this.#canvas.on_pointer_down(x, y, e.button, e.altKey);
    });
    n.canvas.addEventListener('pointerup', e => {
      if (this.#canvas === null) return;
      const { x, y } = this.#cssCoords(e);
      try { n.canvas.releasePointerCapture(e.pointerId); } catch { /* noop */ }
      n.root.className = C.overview;
      this.#canvas.on_pointer_up(x, y, e.button);
    });
    // `pointercancel` fires when the OS / browser steals the gesture
    // (touch scroll, browser drag-out, page navigation). Without this
    // handler the wasm-side `drag` state stayed `Some(...)` and every
    // subsequent pointermove kept orbiting / panning even though the
    // user wasn't holding anything — the "stuck camera" feeling.
    n.canvas.addEventListener('pointercancel', e => {
      if (this.#canvas === null) return;
      try { n.canvas.releasePointerCapture(e.pointerId); } catch { /* noop */ }
      n.root.className = C.overview;
      this.#canvas.on_pointer_leave();
    });
    // Right-click drag should orbit (same as left-click) — without
    // this the browser opens its native context menu mid-drag and the
    // pointerup never reaches our handler, leaving the wasm drag
    // state half-released.
    n.canvas.addEventListener('contextmenu', e => { e.preventDefault(); });
    n.canvas.addEventListener('pointerleave', () => {
      if (this.#canvas === null) return;
      this.#canvas.on_pointer_leave();
    });
    n.canvas.addEventListener('wheel', e => {
      if (this.#canvas === null) return;
      e.preventDefault();
      const { x, y } = this.#cssCoords(e);
      this.#canvas.on_wheel(x, y, e.deltaY);
    }, { passive: false });
  }

  #cssCoords(e: PointerEvent | WheelEvent): { x: number; y: number } {
    const n = this.#nodes;
    if (n === null) return { x: 0, y: 0 };
    const r = n.canvas.getBoundingClientRect();
    return { x: e.clientX - r.left, y: e.clientY - r.top };
  }

  #tryInit(): void {
    if (this.#initStarted) return;
    if (this.#nodes === null || this.#factory === null) return;
    this.#initStarted = true;
    const n = this.#nodes;
    const rect = n.canvas.getBoundingClientRect();
    const w = Math.max(1, Math.round(rect.width || 320));
    const h = Math.max(1, Math.round(rect.height || 240));
    const dpr = Math.max(1, Math.round(window.devicePixelRatio || 1));
    void Promise.resolve(this.#factory(n.canvas, w, h, dpr))
      .then(canvas => {
        this.#canvas = canvas;
        // Sync the persisted cross-edges preference into the freshly
        // bound canvas so the toggle survives reloads without the user
        // having to flip it again.
        canvas.set_show_cross_edges(this.#showCrossEdges);
        // Wire wasm-side hover + click callbacks to DOM events.
        // Without this the cluster-click in the overview canvas
        // fired into the void and the shell never saw the drill
        // signal — which surfaced as 'the overview never moves'
        // because no focus shift was ever requested from there.
        // Wasm-bindgen takes a mutable borrow of the canvas for the
        // full duration of any `&mut self` method (on_pointer_up,
        // on_pointer_move, …). Dispatching synchronously here would
        // let a host listener re-enter wasm (e.g. `set_payload` after
        // a cluster-click drill) WHILE that borrow is still live —
        // wasm-bindgen catches it and throws "recursive use of an
        // object detected". `queueMicrotask` yields after the current
        // wasm call returns, dropping the borrow before the host
        // listener runs. Sub-millisecond delay, imperceptible.
        canvas.set_on_cluster_hover((id: number | null) => {
          queueMicrotask(() => {
            this.dispatchEvent(new CustomEvent<OverviewClusterHoverDetail>('sd-overview-cluster-hover', {
              detail: { clusterId: id }, bubbles: true, composed: true,
            }));
          });
        });
        canvas.set_on_cluster_click((id: number) => {
          queueMicrotask(() => {
            this.dispatchEvent(new CustomEvent<OverviewClusterClickDetail>('sd-overview-cluster-click', {
              detail: { clusterId: id }, bubbles: true, composed: true,
            }));
          });
        });
        // Observe the ROOT wrapper, not the canvas itself. The canvas
        // carries an inline `width: ${px}` set by `apply_canvas_size`
        // (Rust pin to dodge the DPR-bitmap→intrinsic-size resize
        // loop), which makes the canvas size NOT track its parent
        // automatically. The root is `width: 100% / height: 100%` of
        // its grid slot, so observing it catches every layout reflow
        // (panel toggle, resizer drag, window resize).
        this.#observer = new ResizeObserver(() => this.#syncSize());
        this.#observer.observe(n.root);
        this.dispatchEvent(new CustomEvent<OverviewReadyDetail>('sd-overview-ready', {
          detail: { canvas }, bubbles: true, composed: true,
        }));
        this.#loop();
      })
      .catch((e: unknown) => {
        const message = e instanceof Error ? e.message : String(e);
        this.dispatchEvent(new CustomEvent<OverviewErrorDetail>('sd-overview-error', {
          detail: { source: 'canvas-init', message }, bubbles: true, composed: true,
        }));
      });
  }

  #syncSize(): void {
    const n = this.#nodes;
    if (n === null || this.#canvas === null) return;
    // Use the root's rect, not the canvas's: the canvas inline size
    // is pinned by `apply_canvas_size` and lags the parent's reflow
    // until we resize it ourselves.
    const rect = n.root.getBoundingClientRect();
    const w = Math.max(1, Math.round(rect.width));
    const h = Math.max(1, Math.round(rect.height));
    this.#canvas.resize(w, h);
    const dpr = Math.max(1, Math.round(window.devicePixelRatio || 1));
    this.#canvas.set_device_pixel_ratio(dpr);
  }

  #loop(): void {
    if (this.#canvas === null) return;
    this.#applyKeyboardNav();
    this.#canvas.tick();
    this.#syncOrbitBall();
    this.#rafHandle = requestAnimationFrame(() => this.#loop());
  }
}

if (typeof customElements !== 'undefined' && !customElements.get(STANDARDOC_OVERVIEW_TAG)) {
  customElements.define(STANDARDOC_OVERVIEW_TAG, OverviewElement);
}

declare global {
  interface HTMLElementTagNameMap {
    [STANDARDOC_OVERVIEW_TAG]: OverviewElement;
  }
}

// Re-export click/hover detail types for host convenience.
export type {
  OverviewClusterClickDetail,
  OverviewClusterHoverDetail,
};

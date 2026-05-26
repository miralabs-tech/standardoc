/**
 * `<standardoc-select>` — themed combobox replacement for the native
 * `<select>` element. The native dropdown is UA-rendered (Chrome /
 * Firefox style the open menu themselves, ignoring CSS custom props)
 * which surfaces as white-on-light-grey inside the dark shell.
 *
 * Owns:
 *   - a `<button>` trigger styled like the legacy `<select>` closed
 *     state (so visual continuity with the gizmo is preserved)
 *   - a `<ul role="listbox">` popover with options
 *   - keyboard nav (Enter / Space / ArrowUp / ArrowDown / Home / End /
 *     Esc / Tab) and outside-click dismissal
 *
 * Does NOT own:
 *   - host policy (the host decides what `value` maps to)
 *   - placement collision against arbitrary scroll containers (we only
 *     check the viewport; nested overflow is out of scope for V1)
 *
 * Events emitted:
 *   - `sd-select-change` detail: { value, option } — on user pick
 *
 * Properties (set via JS, not attributes — values can be numbers):
 *   - options:     ReadonlyArray<SelectOption>
 *   - value:       string | number | null
 *   - placeholder: string  (shown when value is null / not in options)
 *   - placement:   'top' | 'bottom' | 'auto'  (default 'auto')
 *   - open:        boolean (programmatic open/close)
 *   - disabled:    boolean
 */

import classigo from 'classigo';

import type {
  SelectChangeDetail,
  SelectOption,
  SelectPlacement,
} from './select.type';
import s from './select.module.scss';

export const STANDARDOC_SELECT_TAG = 'standardoc-select';

const C = {
  select: s.select ?? '',
  open: s['select--open'] ?? '',
  button: s.select__button ?? '',
  value: s.select__value ?? '',
  valuePlaceholder: s['select__value--placeholder'] ?? '',
  chevron: s.select__chevron ?? '',
  popover: s.select__popover ?? '',
  popoverTop: s['select__popover--top'] ?? '',
  option: s.select__option ?? '',
  optionActive: s['select__option--active'] ?? '',
  optionSelected: s['select__option--selected'] ?? '',
  optionDisabled: s['select__option--disabled'] ?? '',
} as const;

export class SelectElement extends HTMLElement {
  #mounted = false;
  #options: ReadonlyArray<SelectOption> = [];
  #value: string | number | null = null;
  #placeholder = '';
  #placement: SelectPlacement = 'auto';
  #disabled = false;
  #open = false;
  #highlight = -1;
  #nodes: {
    root: HTMLElement;
    button: HTMLButtonElement;
    value: HTMLElement;
    popover: HTMLElement;
    optionEls: HTMLElement[];
  } | null = null;
  #onDocPointerDown: ((e: PointerEvent) => void) | null = null;

  connectedCallback(): void {
    if (this.#mounted) return;
    this.#mounted = true;
    this.#render();
    this.#syncValue();
    this.#syncDisabled();
  }

  disconnectedCallback(): void {
    this.#removeDocListener();
  }

  set options(value: ReadonlyArray<SelectOption>) {
    this.#options = value;
    if (this.#value !== null && !value.some(o => o.value === this.#value)) {
      this.#value = null;
    }
    this.#renderOptions();
    this.#syncValue();
  }

  get options(): ReadonlyArray<SelectOption> {
    return this.#options;
  }

  set value(v: string | number | null) {
    if (this.#value === v) return;
    this.#value = v;
    this.#syncValue();
  }

  get value(): string | number | null {
    return this.#value;
  }

  set placeholder(v: string) {
    if (this.#placeholder === v) return;
    this.#placeholder = v;
    this.#syncValue();
  }

  get placeholder(): string {
    return this.#placeholder;
  }

  set placement(p: SelectPlacement) {
    this.#placement = p;
  }

  get placement(): SelectPlacement {
    return this.#placement;
  }

  set disabled(v: boolean) {
    if (this.#disabled === v) return;
    this.#disabled = v;
    if (v && this.#open) this.#closePopover();
    this.#syncDisabled();
  }

  get disabled(): boolean {
    return this.#disabled;
  }

  set open(v: boolean) {
    if (this.#open === v) return;
    if (v) this.#openPopover();
    else this.#closePopover();
  }

  get open(): boolean {
    return this.#open;
  }

  #render(): void {
    const root = document.createElement('div');
    root.className = C.select;
    const button = document.createElement('button');
    button.type = 'button';
    button.className = C.button;
    button.setAttribute('aria-haspopup', 'listbox');
    button.setAttribute('aria-expanded', 'false');
    const value = document.createElement('span');
    value.className = C.value;
    const chevron = document.createElement('span');
    chevron.className = C.chevron;
    chevron.textContent = '▾';
    chevron.setAttribute('aria-hidden', 'true');
    button.append(value, chevron);
    const popover = document.createElement('ul');
    popover.className = C.popover;
    popover.setAttribute('role', 'listbox');
    popover.hidden = true;
    root.append(button, popover);
    this.replaceChildren(root);
    this.#nodes = { root, button, value, popover, optionEls: [] };
    this.#wireEvents();
    this.#renderOptions();
  }

  #renderOptions(): void {
    const n = this.#nodes;
    if (n === null) return;
    const els: HTMLElement[] = [];
    const frag = document.createDocumentFragment();
    this.#options.forEach((opt, i) => {
      const li = document.createElement('li');
      li.className = classigo(C.option, opt.disabled === true && C.optionDisabled);
      li.setAttribute('role', 'option');
      li.dataset['value'] = String(opt.value);
      li.dataset['index'] = String(i);
      li.textContent = opt.label;
      if (opt.disabled === true) li.setAttribute('aria-disabled', 'true');
      // Prevent pointerdown from blurring the button before the click
      // handler runs — keeps focus state predictable so Enter/Esc work
      // immediately after a mouse pick.
      li.addEventListener('pointerdown', e => { e.preventDefault(); });
      li.addEventListener('click', () => {
        if (opt.disabled === true) return;
        this.#selectIndex(i, true);
      });
      li.addEventListener('mouseenter', () => {
        if (opt.disabled === true) return;
        this.#setHighlight(i);
      });
      frag.appendChild(li);
      els.push(li);
    });
    n.popover.replaceChildren(frag);
    n.optionEls = els;
    this.#syncSelectedClass();
  }

  #wireEvents(): void {
    const n = this.#nodes;
    if (n === null) return;
    n.button.addEventListener('click', () => {
      if (this.#disabled) return;
      if (this.#open) this.#closePopover();
      else this.#openPopover();
    });
    n.button.addEventListener('keydown', e => this.#onKeyDown(e));
    n.popover.addEventListener('keydown', e => this.#onKeyDown(e));
  }

  #onKeyDown(e: KeyboardEvent): void {
    if (this.#disabled) return;
    if (!this.#open) {
      if (e.key === 'Enter' || e.key === ' ' || e.key === 'ArrowDown' || e.key === 'ArrowUp') {
        e.preventDefault();
        this.#openPopover();
      }
      return;
    }
    if (e.key === 'Escape') {
      e.preventDefault();
      this.#closePopover();
      this.#nodes?.button.focus();
      return;
    }
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      this.#moveHighlight(1);
      return;
    }
    if (e.key === 'ArrowUp') {
      e.preventDefault();
      this.#moveHighlight(-1);
      return;
    }
    if (e.key === 'Home') {
      e.preventDefault();
      this.#setHighlight(this.#firstEnabledIndex());
      return;
    }
    if (e.key === 'End') {
      e.preventDefault();
      this.#setHighlight(this.#lastEnabledIndex());
      return;
    }
    if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault();
      if (this.#highlight >= 0) this.#selectIndex(this.#highlight, true);
      return;
    }
    if (e.key === 'Tab') {
      this.#closePopover();
    }
  }

  #moveHighlight(dir: 1 | -1): void {
    const len = this.#options.length;
    if (len === 0) return;
    let i = this.#highlight;
    if (i < 0) i = dir === 1 ? -1 : 0;
    for (let step = 0; step < len; step += 1) {
      i = (i + dir + len) % len;
      const opt = this.#options[i];
      if (opt !== undefined && opt.disabled !== true) {
        this.#setHighlight(i);
        return;
      }
    }
  }

  #firstEnabledIndex(): number {
    for (let i = 0; i < this.#options.length; i += 1) {
      if (this.#options[i]?.disabled !== true) return i;
    }
    return -1;
  }

  #lastEnabledIndex(): number {
    for (let i = this.#options.length - 1; i >= 0; i -= 1) {
      if (this.#options[i]?.disabled !== true) return i;
    }
    return -1;
  }

  #setHighlight(i: number): void {
    if (this.#highlight === i) return;
    this.#highlight = i;
    this.#syncHighlightClass();
    const el = this.#nodes?.optionEls[i];
    if (el !== undefined) {
      el.scrollIntoView({ block: 'nearest' });
    }
  }

  #syncHighlightClass(): void {
    const n = this.#nodes;
    if (n === null) return;
    n.optionEls.forEach((el, idx) => {
      el.classList.toggle(C.optionActive, idx === this.#highlight);
    });
  }

  #syncSelectedClass(): void {
    const n = this.#nodes;
    if (n === null) return;
    n.optionEls.forEach((el, idx) => {
      const opt = this.#options[idx];
      const isSelected = opt !== undefined && opt.value === this.#value;
      el.classList.toggle(C.optionSelected, isSelected);
      if (isSelected) el.setAttribute('aria-selected', 'true');
      else el.removeAttribute('aria-selected');
    });
  }

  #syncValue(): void {
    const n = this.#nodes;
    if (n === null) return;
    const opt = this.#options.find(o => o.value === this.#value);
    if (opt !== undefined) {
      n.value.textContent = opt.label;
      n.value.className = C.value;
    } else {
      n.value.textContent = this.#placeholder;
      n.value.className = classigo(C.value, C.valuePlaceholder);
    }
    this.#syncSelectedClass();
  }

  #syncDisabled(): void {
    const n = this.#nodes;
    if (n === null) return;
    n.button.disabled = this.#disabled;
  }

  #selectIndex(i: number, emit: boolean): void {
    const opt = this.#options[i];
    if (opt === undefined || opt.disabled === true) return;
    const changed = this.#value !== opt.value;
    this.#value = opt.value;
    this.#syncValue();
    this.#closePopover();
    this.#nodes?.button.focus();
    if (emit && changed) {
      this.dispatchEvent(new CustomEvent<SelectChangeDetail>('sd-select-change', {
        detail: { value: opt.value, option: opt },
        bubbles: true,
        composed: true,
      }));
    }
  }

  #openPopover(): void {
    if (this.#open || this.#disabled) return;
    const n = this.#nodes;
    if (n === null) return;
    this.#open = true;
    n.popover.hidden = false;
    n.button.setAttribute('aria-expanded', 'true');
    n.root.classList.add(C.open);
    this.#resolvePlacement();
    const selectedIdx = this.#options.findIndex(o => o.value === this.#value);
    this.#setHighlight(selectedIdx >= 0 ? selectedIdx : this.#firstEnabledIndex());
    this.#installDocListener();
  }

  #closePopover(): void {
    if (!this.#open) return;
    const n = this.#nodes;
    if (n === null) return;
    this.#open = false;
    n.popover.hidden = true;
    n.button.setAttribute('aria-expanded', 'false');
    n.root.classList.remove(C.open);
    this.#highlight = -1;
    this.#syncHighlightClass();
    this.#removeDocListener();
  }

  #resolvePlacement(): void {
    const n = this.#nodes;
    if (n === null) return;
    let resolved: 'top' | 'bottom';
    if (this.#placement === 'top' || this.#placement === 'bottom') {
      resolved = this.#placement;
    } else {
      const r = n.button.getBoundingClientRect();
      const spaceBelow = window.innerHeight - r.bottom;
      const popoverHeight = Math.min(240, this.#options.length * 28 + 8);
      resolved = spaceBelow < popoverHeight && r.top > popoverHeight ? 'top' : 'bottom';
    }
    n.popover.classList.toggle(C.popoverTop, resolved === 'top');
  }

  #installDocListener(): void {
    if (this.#onDocPointerDown !== null) return;
    this.#onDocPointerDown = (e: PointerEvent) => {
      const t = e.target;
      if (t instanceof Node && this.contains(t)) return;
      this.#closePopover();
    };
    document.addEventListener('pointerdown', this.#onDocPointerDown, true);
  }

  #removeDocListener(): void {
    if (this.#onDocPointerDown === null) return;
    document.removeEventListener('pointerdown', this.#onDocPointerDown, true);
    this.#onDocPointerDown = null;
  }
}

if (typeof customElements !== 'undefined' && !customElements.get(STANDARDOC_SELECT_TAG)) {
  customElements.define(STANDARDOC_SELECT_TAG, SelectElement);
}

declare global {
  interface HTMLElementTagNameMap {
    [STANDARDOC_SELECT_TAG]: SelectElement;
  }
}

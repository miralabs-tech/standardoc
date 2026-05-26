// Cross-cutting types shared by every component.
export type { RenderMode, Status, StatusKind } from './types';

// Each module barrels its own `.type.ts` + `.element.ts` (or `.ts` for
// the profiler) so this top-level export surface stays terse. Component
// `<custom-element>` definitions register as side effects when the
// matching `.element.ts` is imported, which is why `package.json`
// declares them under `sideEffects`.
export * from './components/toolbar';
export * from './components/hud';
export * from './components/panel-layout';
export * from './components/explorer';
export * from './components/symbol-details';
export * from './components/search';
export * from './components/overview';
export * from './components/focus-graph';
export * from './components/compare-panel';
export * from './components/panel-host';
export * from './components/legend';
export * from './components/select';
export * from './profiler';
export * from './mcp-client';
export * from './focus-store';
export * from './panel-manager';

import type { SelectOption } from '../select';
import s from './overview.module.scss';

export const STANDARDOC_OVERVIEW_TAG = 'standardoc-overview';

export const C = {
  overview: s.overview ?? '',
  grabbing: s['overview--grabbing'] ?? '',
  canvas: s.overview__canvas ?? '',
  breadcrumb: s.overview__breadcrumb ?? '',
  breadcrumbBack: s['overview__breadcrumb-back'] ?? '',
  breadcrumbSep: s['overview__breadcrumb-sep'] ?? '',
  breadcrumbCurrent: s['overview__breadcrumb-current'] ?? '',
  gizmo: s.overview__gizmo ?? '',
  gizmoRow: s['overview__gizmo-row'] ?? '',
  gizmoGroup: s['overview__gizmo-group'] ?? '',
  gizmoSep: s['overview__gizmo-sep'] ?? '',
  gizmoBtn: s['overview__gizmo-btn'] ?? '',
  gizmoBtnActive: s['overview__gizmo-btn--active'] ?? '',
  gizmoLabel: s['overview__gizmo-label'] ?? '',
  gizmoSelect: s['overview__gizmo-select'] ?? '',
  legend: s.overview__legend ?? '',
  legendEmpty: s['overview__legend--empty'] ?? '',
  legendItem: s['overview__legend-item'] ?? '',
  legendSwatch: s['overview__legend-swatch'] ?? '',
  legendSwatchDashed: s['overview__legend-swatch--dashed'] ?? '',
  ball: s.overview__ball ?? '',
  ballAxis: s['overview__ball-axis'] ?? '',
  ballAxisBehind: s['overview__ball-axis--behind'] ?? '',
  ballTip: s['overview__ball-tip'] ?? '',
  ballLabel: s['overview__ball-label'] ?? '',
} as const;

export const PRESET_BUTTONS: ReadonlyArray<{ preset: string; label: string; title: string }> = [
  { preset: 'orbit', label: 'orb', title: 'Orbit (3/4 view)' },
  { preset: 'top', label: 'top', title: 'Top-down view' },
  { preset: 'front', label: 'fr', title: 'Front view' },
  { preset: 'side', label: 'side', title: 'Side view' },
];

export const LABEL_LIMIT_OPTIONS: ReadonlyArray<SelectOption> = [
  { value: 0, label: 'all' },
  { value: 5, label: '5' },
  { value: 10, label: '10' },
  { value: 20, label: '20' },
];

export const DEFAULT_PRESET = 'orbit';
export const DEFAULT_LABEL_LIMIT = 0;

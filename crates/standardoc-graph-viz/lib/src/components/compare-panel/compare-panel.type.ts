// Compare-panel data shape — two `SymbolDetail` lenses side by side.
// The host produces a `SymbolDetail` per symbol (same shape that feeds
// `<standardoc-symbol-details>`, kept identical so the shell's
// `buildSymbolDetail` helper feeds both panels without translation).

import type { SymbolDetail } from '../symbol-details/symbol-details.type';

export interface CompareSide {
  readonly fqdn: string;
  readonly detail: SymbolDetail | null;
  readonly loading: boolean;
}

export interface ComparePanelData {
  readonly left: CompareSide;
  readonly right: CompareSide;
}

/**
 * Detail of `sd-compare-refresh-request` — fired when the user clicks
 * the panel's refresh button. The host is expected to re-fetch both
 * symbols and push the new data via the `data` setter.
 */
export interface CompareRefreshRequestDetail {
  readonly leftFqdn: string;
  readonly rightFqdn: string;
}

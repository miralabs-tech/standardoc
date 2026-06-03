// Public shapes for `<standardoc-search>`. The host owns the MCP
// fetch (most likely `find_symbols_by_pattern` or `find_symbol`) and
// hands the resolved list back via the `results` property setter. The
// component never speaks to MCP directly — keeps it embeddable in
// hosts that route through a different transport (postMessage in the
// VSCode webview, in-process call in a future native binding).

export interface SymbolSearchResult {
  readonly fqdn: string;
  readonly name: string;
  readonly kindLabel: string;
  /** Canonical IR bucket — `callable | type | value | module | macro`.
   *  Drives the colored kind chip + left border in the dropdown. The
   *  free-text `kindLabel` stays for finer-grained display (e.g.
   *  `interface` vs `struct`, both bucket = `type`). */
  readonly kind?: string;
  /** Source file. Optional so suggestions / did_you_mean items (which
   *  only carry name + fqdn from the daemon) can reuse the same shape. */
  readonly file?: string;
  readonly startLine?: number;
}

/**
 * Lightweight "did you mean…" suggestion surfaced when a query
 * returns zero direct matches. The daemon's `find_symbol` switches
 * its response to `{ results: [], did_you_mean: [...] }` based on
 * strsim, threshold 0.6. The shell pushes these through here so the
 * dropdown can render a fallback list instead of a bare "No results".
 */
export interface SymbolSearchSuggestion {
  readonly fqdn: string;
  readonly name: string;
  readonly kindLabel: string;
  readonly kind?: string;
}

export interface SearchQueryDetail {
  readonly query: string;
}

export interface SearchSelectDetail {
  readonly fqdn: string;
}

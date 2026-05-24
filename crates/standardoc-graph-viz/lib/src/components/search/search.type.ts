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
	readonly file: string;
	readonly startLine: number;
}

export interface SearchQueryDetail {
	readonly query: string;
}

export interface SearchSelectDetail {
	readonly fqdn: string;
}

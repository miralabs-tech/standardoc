/**
 * Tiny text helpers shared across the shell, stores, and web-components.
 * Kept in a leaf module (no intra-package imports) so any layer — even a
 * low-level store — can use them without importing "upward".
 */

/** Last `::`-separated segment of an fqdn (the bare symbol / module name). */
export function shortFqdn(fqdn: string): string {
  const idx = fqdn.lastIndexOf('::');
  return idx >= 0 ? fqdn.slice(idx + 2) : fqdn;
}

/**
 * Escape the HTML-significant characters before interpolating untrusted
 * text into an `innerHTML` string (text content + double-quoted attribute
 * values). Single source so a future hardening can't be applied to some
 * call sites and missed on others.
 */
export function escapeHtml(text: string): string {
  return text
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;');
}

import { C } from './symbol-details.constants';

/**
 * Coarse test-symbol detector mirroring the Rust
 * `query::symbol_looks_like_test` heuristic so the panel's "Hide tests"
 * toggle matches what the MCP `exclude_tests` flag does server-side.
 * Catches Rust `::tests::` / `::test::` modules, `_test` / `_tests`
 * suffixes, `tests/` directories, `*_test.rs`; TS `.test.ts` /
 * `.spec.ts` (+ `.tsx`, `.js`, `.jsx`), `__tests__/` dirs.
 */
export function looksLikeTest(fqdn: string, file?: string | null): boolean {
  if (fqdn.includes('::tests::') || fqdn.includes('::test::')) return true;
  if (fqdn.endsWith('::tests') || fqdn.endsWith('::test')) return true;
  if (fqdn.endsWith('_test') || fqdn.endsWith('_tests')) return true;
  if (fqdn.endsWith('.test') || fqdn.endsWith('.spec')) return true;
  if (!file) return false;
  const norm = file.replace(/\\/g, '/');
  if (norm.includes('/tests/') || norm.includes('/test/') || norm.includes('/__tests__/')) return true;
  if (/(?:_tests?\.rs|\.(?:test|spec)\.(?:tsx?|jsx?))$/.test(norm)) return true;
  return false;
}

export function shortFqdn(fqdn: string): string {
  const idx = fqdn.lastIndexOf('::');
  return idx >= 0 ? fqdn.slice(idx + 2) : fqdn;
}

const KIND_CALLABLE = new Set(['function', 'fn', 'method', 'impl_fn', 'trait_fn', 'interface_method', 'getter', 'setter', 'constructor']);
const KIND_TYPE = new Set(['struct', 'enum', 'class', 'interface', 'trait', 'type_alias', 'union']);
const KIND_VALUE = new Set(['const', 'static', 'let', 'var', 'field', 'enum_variant', 'property', 'interface_property']);
const KIND_MODULE = new Set(['module', 'namespace', 'package', 'crate']);
const KIND_MACRO = new Set(['macro', 'macro_rules', 'proc_macro', 'decorator', 'declarativemacro', 'procmacro']);

export function kindFamilyTagClass(kindLabel: string): string {
  const k = kindLabel.toLowerCase();
  if (KIND_CALLABLE.has(k)) return C.tagKindCallable;
  if (KIND_TYPE.has(k)) return C.tagKindType;
  if (KIND_VALUE.has(k)) return C.tagKindValue;
  if (KIND_MODULE.has(k)) return C.tagKindModule;
  if (KIND_MACRO.has(k)) return C.tagKindMacro;
  return '';
}

export function visibilityTagClass(visibility: string): string {
  switch (visibility.toLowerCase()) {
    case 'public': return C.tagVisPublic;
    case 'private': return C.tagVisPrivate;
    case 'crate': return C.tagVisCrate;
    case 'protected': return C.tagVisProtected;
    default: return '';
  }
}

export function escapeHtml(text: string): string {
  return text
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;');
}

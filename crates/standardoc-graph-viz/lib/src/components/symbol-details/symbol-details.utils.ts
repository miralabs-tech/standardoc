import { C } from './symbol-details.constants';
import { kindFamily } from '../../kind-family';

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

export function kindFamilyTagClass(kindLabel: string): string {
  switch (kindFamily(kindLabel)) {
    case 'callable': return C.tagKindCallable;
    case 'type': return C.tagKindType;
    case 'value': return C.tagKindValue;
    case 'module': return C.tagKindModule;
    case 'macro': return C.tagKindMacro;
    default: return '';
  }
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

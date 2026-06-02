import { C } from './symbol-details.constants';
import { kindFamily } from '../../kind-family';

/**
 * Fqdn-only test-symbol detector for the Symbol Details Relations
 * section. Relation rows are synthesised from get_context edges and
 * carry no file path, so only the fqdn heuristic applies. The
 * list_symbols / fetch_graph paths read the daemon's `is_test` verdict
 * instead (no client-side re-derivation); this fqdn fallback survives
 * until get_context surfaces `is_test` on its edge targets too.
 */
export function looksLikeTest(fqdn: string): boolean {
  if (fqdn.includes('::tests::') || fqdn.includes('::test::')) return true;
  if (fqdn.endsWith('::tests') || fqdn.endsWith('::test')) return true;
  if (fqdn.endsWith('_test') || fqdn.endsWith('_tests')) return true;
  if (fqdn.endsWith('.test') || fqdn.endsWith('.spec')) return true;
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

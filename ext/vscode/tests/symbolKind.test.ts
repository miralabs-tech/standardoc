import { describe, expect, test } from 'bun:test';
import {
  SYMBOL_KIND_FALLBACK_ID,
  SYMBOL_KIND_RULES,
  themeIdForSymbolKind,
} from '../src/symbol-kind-map';

const NAMED_RULES = SYMBOL_KIND_RULES.filter(
  (r): r is { with: number; then: string } => 'with' in r,
);

describe('themeIdForSymbolKind — exhaustive 26 named rules', () => {
  for (const rule of NAMED_RULES) {
    test(`kind ${rule.with} → ${rule.then}`, () => {
      expect(themeIdForSymbolKind(rule.with)).toBe(rule.then);
    });
  }

  test('returns 26 named rules + 1 fallback', () => {
    expect(NAMED_RULES.length).toBe(26);
    expect(SYMBOL_KIND_RULES.length).toBe(27);
  });

  test('falls back to symbol-misc for unknown kind', () => {
    expect(themeIdForSymbolKind(999)).toBe(SYMBOL_KIND_FALLBACK_ID);
  });

  test('falls back to symbol-misc for negative kind', () => {
    expect(themeIdForSymbolKind(-1)).toBe(SYMBOL_KIND_FALLBACK_ID);
  });
});

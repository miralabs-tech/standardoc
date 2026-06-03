/**
 * Canonical symbol-kind → family classification, shared by every panel
 * that tints by family (Search result chips, Symbol Details kind tag).
 * One union vocabulary so a kind recognised by one panel isn't silently
 * un-tinted by another (the previous per-panel Sets diverged — e.g. one
 * knew `class_method`, the other didn't). The extractor is the source of
 * the raw kind; this only buckets it for display.
 */
export type KindFamily = 'callable' | 'type' | 'value' | 'module' | 'macro' | 'unknown';

const CALLABLE = new Set([
  'callable', 'function', 'fn', 'method', 'impl_fn', 'trait_fn',
  'interface_method', 'class_method', 'getter', 'setter', 'constructor',
]);
const TYPE = new Set([
  'type', 'struct', 'enum', 'class', 'interface', 'trait', 'type_alias',
  'typedef', 'union',
]);
const VALUE = new Set([
  'value', 'const', 'constant', 'static', 'let', 'var', 'variable', 'field',
  'struct_field', 'class_property', 'enum_variant', 'property',
  'interface_property',
]);
const MODULE = new Set(['module', 'namespace', 'package', 'crate']);
const MACRO = new Set([
  'macro', 'macro_rules', 'proc_macro', 'decorator', 'declarativemacro',
  'procmacro',
]);

/** Bucket a raw kind / decl_kind / language_kind string into its family. */
export function kindFamily(kind: string | undefined | null): KindFamily {
  if (kind === undefined || kind === null) return 'unknown';
  const k = kind.toLowerCase();
  if (CALLABLE.has(k)) return 'callable';
  if (TYPE.has(k)) return 'type';
  if (VALUE.has(k)) return 'value';
  if (MODULE.has(k)) return 'module';
  if (MACRO.has(k)) return 'macro';
  return 'unknown';
}

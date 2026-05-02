import { compile, type Rule } from 'matchigo';

const SYMBOL_KIND_FILE = 0;
const SYMBOL_KIND_MODULE = 1;
const SYMBOL_KIND_NAMESPACE = 2;
const SYMBOL_KIND_PACKAGE = 3;
const SYMBOL_KIND_CLASS = 4;
const SYMBOL_KIND_METHOD = 5;
const SYMBOL_KIND_PROPERTY = 6;
const SYMBOL_KIND_FIELD = 7;
const SYMBOL_KIND_CONSTRUCTOR = 8;
const SYMBOL_KIND_ENUM = 9;
const SYMBOL_KIND_INTERFACE = 10;
const SYMBOL_KIND_FUNCTION = 11;
const SYMBOL_KIND_VARIABLE = 12;
const SYMBOL_KIND_CONSTANT = 13;
const SYMBOL_KIND_STRING = 14;
const SYMBOL_KIND_NUMBER = 15;
const SYMBOL_KIND_BOOLEAN = 16;
const SYMBOL_KIND_ARRAY = 17;
const SYMBOL_KIND_OBJECT = 18;
const SYMBOL_KIND_KEY = 19;
const SYMBOL_KIND_NULL = 20;
const SYMBOL_KIND_ENUM_MEMBER = 21;
const SYMBOL_KIND_STRUCT = 22;
const SYMBOL_KIND_EVENT = 23;
const SYMBOL_KIND_OPERATOR = 24;
const SYMBOL_KIND_TYPE_PARAMETER = 25;

export const SYMBOL_KIND_FALLBACK_ID = 'symbol-misc';

export const SYMBOL_KIND_RULES: ReadonlyArray<Rule<number, string>> = [
  { with: SYMBOL_KIND_FILE, then: 'symbol-file' },
  { with: SYMBOL_KIND_MODULE, then: 'symbol-module' },
  { with: SYMBOL_KIND_NAMESPACE, then: 'symbol-namespace' },
  { with: SYMBOL_KIND_PACKAGE, then: 'symbol-package' },
  { with: SYMBOL_KIND_CLASS, then: 'symbol-class' },
  { with: SYMBOL_KIND_METHOD, then: 'symbol-method' },
  { with: SYMBOL_KIND_PROPERTY, then: 'symbol-property' },
  { with: SYMBOL_KIND_FIELD, then: 'symbol-field' },
  { with: SYMBOL_KIND_CONSTRUCTOR, then: 'symbol-constructor' },
  { with: SYMBOL_KIND_ENUM, then: 'symbol-enum' },
  { with: SYMBOL_KIND_INTERFACE, then: 'symbol-interface' },
  { with: SYMBOL_KIND_FUNCTION, then: 'symbol-function' },
  { with: SYMBOL_KIND_VARIABLE, then: 'symbol-variable' },
  { with: SYMBOL_KIND_CONSTANT, then: 'symbol-constant' },
  { with: SYMBOL_KIND_STRING, then: 'symbol-string' },
  { with: SYMBOL_KIND_NUMBER, then: 'symbol-number' },
  { with: SYMBOL_KIND_BOOLEAN, then: 'symbol-boolean' },
  { with: SYMBOL_KIND_ARRAY, then: 'symbol-array' },
  { with: SYMBOL_KIND_OBJECT, then: 'symbol-object' },
  { with: SYMBOL_KIND_KEY, then: 'symbol-key' },
  { with: SYMBOL_KIND_NULL, then: 'symbol-null' },
  { with: SYMBOL_KIND_ENUM_MEMBER, then: 'symbol-enum-member' },
  { with: SYMBOL_KIND_STRUCT, then: 'symbol-struct' },
  { with: SYMBOL_KIND_EVENT, then: 'symbol-event' },
  { with: SYMBOL_KIND_OPERATOR, then: 'symbol-operator' },
  { with: SYMBOL_KIND_TYPE_PARAMETER, then: 'symbol-type-parameter' },
  { otherwise: SYMBOL_KIND_FALLBACK_ID },
];

export const themeIdForSymbolKind = compile<number, string>(SYMBOL_KIND_RULES);

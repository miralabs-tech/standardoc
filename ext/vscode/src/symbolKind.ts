import * as vscode from 'vscode';
import { compile, type Rule } from 'matchigo';

const RULES: ReadonlyArray<Rule<vscode.SymbolKind, vscode.ThemeIcon>> = [
  { with: vscode.SymbolKind.File, then: new vscode.ThemeIcon('symbol-file') },
  { with: vscode.SymbolKind.Module, then: new vscode.ThemeIcon('symbol-module') },
  { with: vscode.SymbolKind.Namespace, then: new vscode.ThemeIcon('symbol-namespace') },
  { with: vscode.SymbolKind.Package, then: new vscode.ThemeIcon('symbol-package') },
  { with: vscode.SymbolKind.Class, then: new vscode.ThemeIcon('symbol-class') },
  { with: vscode.SymbolKind.Method, then: new vscode.ThemeIcon('symbol-method') },
  { with: vscode.SymbolKind.Property, then: new vscode.ThemeIcon('symbol-property') },
  { with: vscode.SymbolKind.Field, then: new vscode.ThemeIcon('symbol-field') },
  { with: vscode.SymbolKind.Constructor, then: new vscode.ThemeIcon('symbol-constructor') },
  { with: vscode.SymbolKind.Enum, then: new vscode.ThemeIcon('symbol-enum') },
  { with: vscode.SymbolKind.Interface, then: new vscode.ThemeIcon('symbol-interface') },
  { with: vscode.SymbolKind.Function, then: new vscode.ThemeIcon('symbol-function') },
  { with: vscode.SymbolKind.Variable, then: new vscode.ThemeIcon('symbol-variable') },
  { with: vscode.SymbolKind.Constant, then: new vscode.ThemeIcon('symbol-constant') },
  { with: vscode.SymbolKind.String, then: new vscode.ThemeIcon('symbol-string') },
  { with: vscode.SymbolKind.Number, then: new vscode.ThemeIcon('symbol-number') },
  { with: vscode.SymbolKind.Boolean, then: new vscode.ThemeIcon('symbol-boolean') },
  { with: vscode.SymbolKind.Array, then: new vscode.ThemeIcon('symbol-array') },
  { with: vscode.SymbolKind.Object, then: new vscode.ThemeIcon('symbol-object') },
  { with: vscode.SymbolKind.Key, then: new vscode.ThemeIcon('symbol-key') },
  { with: vscode.SymbolKind.Null, then: new vscode.ThemeIcon('symbol-null') },
  { with: vscode.SymbolKind.EnumMember, then: new vscode.ThemeIcon('symbol-enum-member') },
  { with: vscode.SymbolKind.Struct, then: new vscode.ThemeIcon('symbol-struct') },
  { with: vscode.SymbolKind.Event, then: new vscode.ThemeIcon('symbol-event') },
  { with: vscode.SymbolKind.Operator, then: new vscode.ThemeIcon('symbol-operator') },
  { with: vscode.SymbolKind.TypeParameter, then: new vscode.ThemeIcon('symbol-type-parameter') },
  { otherwise: new vscode.ThemeIcon('symbol-misc') },
];

export const iconForSymbolKind = compile<vscode.SymbolKind, vscode.ThemeIcon>(RULES);

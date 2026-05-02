import * as vscode from 'vscode';
import { themeIdForSymbolKind } from './symbol-kind-map';

export const iconForSymbolKind = (kind: vscode.SymbolKind): vscode.ThemeIcon =>
  new vscode.ThemeIcon(themeIdForSymbolKind(kind));

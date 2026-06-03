import * as vscode from 'vscode';
import { McpClient } from '../mcp/client';
import { GraphPanel } from '../webview/graph-panel';

/**
 * Open the graph-viz shell in an embedded webview, talking to the live
 * MCP daemon through a postMessage frame relay (see ../webview/relay).
 *
 * Replaces the previous behaviour, which spawned the standalone
 * playground dev server in a terminal and opened a browser — that
 * required the user to be editing the standardoc repo itself and ran
 * the shell out-of-process. The webview hosts the same shell in-editor
 * against whatever workspace the daemon is indexing.
 */
export function openGraphViz(
  context: vscode.ExtensionContext,
  mcp: McpClient,
  output: vscode.OutputChannel,
): void {
  GraphPanel.createOrShow(context, mcp, output);
}

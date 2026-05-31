import * as vscode from 'vscode';
import { StreamableHTTPClientTransport } from '@modelcontextprotocol/sdk/client/streamableHttp.js';
import type { JSONRPCMessage } from '@modelcontextprotocol/sdk/types.js';

// Host-side half of the webview MCP bridge. Owns its OWN streamable-HTTP
// transport to the daemon (separate from the extension's `McpClient`
// connection so the two don't interleave on one session) and relays
// raw JSON-RPC frames between the webview and the daemon:
//
//   webview  --postMessage({type:'mcp',frame})-->  relay.send -> daemon
//   daemon   --onmessage(frame)-->  relay -> webview.postMessage
//
// The webview's MCP Client drives the initialize handshake end-to-end
// through this conduit; the relay never parses or rewrites a frame.

interface McpEnvelope {
  readonly type: 'mcp';
  readonly frame: JSONRPCMessage;
}

function isMcpEnvelope(value: unknown): value is McpEnvelope {
  return (
    typeof value === 'object' &&
    value !== null &&
    (value as { type?: unknown }).type === 'mcp' &&
    'frame' in value
  );
}

export class WebviewMcpRelay implements vscode.Disposable {
  private transport: StreamableHTTPClientTransport | null = null;
  private started: Promise<void> | null = null;
  private disposed = false;
  private readonly subscriptions: vscode.Disposable[] = [];

  constructor(
    private readonly webview: vscode.Webview,
    private readonly endpointUrl: string,
    private readonly output: vscode.OutputChannel,
  ) {
    // Wire the webview→daemon direction synchronously at construction so
    // no frame the webview emits is dropped before we start listening.
    this.subscriptions.push(
      webview.onDidReceiveMessage((msg: unknown) => {
        if (isMcpEnvelope(msg)) void this.forwardToDaemon(msg.frame);
      }),
    );
  }

  /** Lazily open the daemon transport on the first frame. */
  private ensureStarted(): Promise<void> {
    if (this.started !== null) return this.started;
    const transport = new StreamableHTTPClientTransport(new URL(this.endpointUrl));
    transport.onmessage = (frame: JSONRPCMessage): void => {
      void this.webview.postMessage({ type: 'mcp', frame } satisfies McpEnvelope);
    };
    transport.onerror = (err: Error): void => {
      this.output.appendLine(`[viz-relay] daemon transport error: ${err.message}`);
    };
    transport.onclose = (): void => {
      this.output.appendLine('[viz-relay] daemon transport closed');
    };
    this.transport = transport;
    this.started = transport.start();
    return this.started;
  }

  private async forwardToDaemon(frame: JSONRPCMessage): Promise<void> {
    if (this.disposed) return;
    try {
      await this.ensureStarted();
      await this.transport?.send(frame);
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      this.output.appendLine(`[viz-relay] forward failed: ${msg}`);
    }
  }

  dispose(): void {
    this.disposed = true;
    for (const s of this.subscriptions) s.dispose();
    this.subscriptions.length = 0;
    void this.transport?.close();
    this.transport = null;
  }
}

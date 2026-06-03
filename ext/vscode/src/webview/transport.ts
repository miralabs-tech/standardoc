// Webview-side MCP transport. Runs INSIDE the graph webview and is
// handed to `mountShell` → `McpBrowse.connect`. The daemon's HTTP
// endpoint isn't reachable from the `vscode-webview://` origin (no
// CORS, sandboxed fetch), so every JSON-RPC frame is tunnelled to the
// extension host over `postMessage`; the host relay (see relay.ts)
// forwards it to the daemon and ships responses back the same way.
//
// This is a pure frame conduit: the real MCP Client (in the webview)
// handshakes end-to-end with the daemon through the pipe. The host
// reinterprets nothing — `viz = dumb renderer`.

import type { Transport } from '@modelcontextprotocol/sdk/shared/transport.js';
import type { JSONRPCMessage } from '@modelcontextprotocol/sdk/types.js';

interface VsCodeApi {
  postMessage(message: unknown): void;
}

declare function acquireVsCodeApi(): VsCodeApi;

/** Frames the host and webview exchange over postMessage. */
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

export class WebviewClientTransport implements Transport {
  onclose?: () => void;
  onerror?: (error: Error) => void;
  onmessage?: (message: JSONRPCMessage) => void;
  sessionId?: string;

  private readonly vscode: VsCodeApi;
  private readonly listener: (event: MessageEvent) => void;

  constructor() {
    this.vscode = acquireVsCodeApi();
    this.listener = (event: MessageEvent): void => {
      const data: unknown = event.data;
      if (isMcpEnvelope(data)) {
        this.onmessage?.(data.frame);
      }
    };
  }

  async start(): Promise<void> {
    window.addEventListener('message', this.listener);
  }

  async send(message: JSONRPCMessage): Promise<void> {
    this.vscode.postMessage({ type: 'mcp', frame: message } satisfies McpEnvelope);
  }

  async close(): Promise<void> {
    window.removeEventListener('message', this.listener);
    this.onclose?.();
  }
}

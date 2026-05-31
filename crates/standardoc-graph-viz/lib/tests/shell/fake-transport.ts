// Fake MCP server for the shell harness, wired to the client through
// the SDK's own in-memory transport pair. Using the real low-level
// `Server` means the `initialize` handshake + protocol-version
// negotiation are handled by the SDK itself — we only register the
// tool handlers, answered from the fixture table. Browser-safe: the
// in-memory transport carries no Node deps.

import { Server } from '@modelcontextprotocol/sdk/server/index.js';
import { InMemoryTransport } from '@modelcontextprotocol/sdk/inMemory.js';
import {
  CallToolRequestSchema,
  ListToolsRequestSchema,
} from '@modelcontextprotocol/sdk/types.js';
import type { Transport } from '@modelcontextprotocol/sdk/shared/transport.js';

import { resolveTool } from './fixtures';

/**
 * Spin up a fixture-backed MCP server on one end of an in-memory
 * transport pair and return the CLIENT end for `McpBrowse.connect`.
 * The server is connected (and thus ready to handshake) before the
 * client transport is handed back.
 */
export async function createFakeTransport(): Promise<Transport> {
  const server = new Server(
    { name: 'standardoc-shell-harness', version: '0.0.0' },
    { capabilities: { tools: {} } },
  );

  server.setRequestHandler(ListToolsRequestSchema, async () => ({ tools: [] }));
  server.setRequestHandler(CallToolRequestSchema, async req => {
    const args = (req.params.arguments as Record<string, unknown> | undefined) ?? {};
    return resolveTool(req.params.name, args);
  });

  const [clientTransport, serverTransport] = InMemoryTransport.createLinkedPair();
  await server.connect(serverTransport);
  return clientTransport;
}

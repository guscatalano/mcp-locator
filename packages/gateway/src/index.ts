#!/usr/bin/env node
/**
 * mcp-locator gateway — the one MCP server an AI client ever has to be told about.
 *
 * Configure this once (see README) and every MCP server registered on the machine becomes
 * discoverable through it, before and after that client was configured.
 */
import { StdioServerTransport } from '@modelcontextprotocol/sdk/server/stdio.js';
import { Gateway } from './gateway.js';

export { Gateway, aliasFor } from './gateway.js';
export { SocketClientTransport } from './socketTransport.js';

async function main(): Promise<void> {
  const gateway = new Gateway({ autostartBroker: true });

  let stopping = false;
  const shutdown = async () => {
    if (stopping) return; // stdin usually reports both 'end' and 'close'
    stopping = true;
    await gateway.shutdown();
    process.exit(0);
  };
  process.on('SIGINT', shutdown);
  process.on('SIGTERM', shutdown);

  // A stdio server's lifetime is its stdin. When the client goes away — cleanly, by crashing, or
  // by being killed — stdin closes and nothing will ever arrive again. Without this the gateway
  // outlives its client: one was left running after its host process was killed, holding a
  // broker connection and every grant it had taken. Signals do not cover it, because a client
  // that dies never sends one.
  process.stdin.on('end', shutdown);
  process.stdin.on('close', shutdown);

  await gateway.server.connect(new StdioServerTransport());
}

// Only run as a program when invoked directly, so tests can import Gateway without starting it.
if (process.argv[1] && import.meta.url.endsWith(process.argv[1].replace(/\\/g, '/').split('/').pop() ?? '')) {
  main().catch((e) => {
    console.error(`mcp-locator gateway failed to start: ${e.message}`);
    process.exit(1);
  });
}

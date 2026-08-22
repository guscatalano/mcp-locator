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

  const shutdown = async () => {
    await gateway.shutdown();
    process.exit(0);
  };
  process.on('SIGINT', shutdown);
  process.on('SIGTERM', shutdown);

  await gateway.server.connect(new StdioServerTransport());
}

// Only run as a program when invoked directly, so tests can import Gateway without starting it.
if (process.argv[1] && import.meta.url.endsWith(process.argv[1].replace(/\\/g, '/').split('/').pop() ?? '')) {
  main().catch((e) => {
    console.error(`mcp-locator gateway failed to start: ${e.message}`);
    process.exit(1);
  });
}

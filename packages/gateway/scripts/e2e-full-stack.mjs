#!/usr/bin/env node
/**
 * Full stack, exactly as a real AI client would drive it:
 *
 *   MCP client -> gateway (stdio) -> broker (pipe) -> MCP server (relay pipe)
 *
 * Requires a running broker and a registered, consented server. See README "Try it".
 *
 *   node packages/gateway/scripts/e2e-full-stack.mjs com.example.echo
 */
import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { StdioClientTransport } from '@modelcontextprotocol/sdk/client/stdio.js';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const target = process.argv[2] ?? 'com.example.echo';
const gatewayEntry = join(dirname(fileURLToPath(import.meta.url)), '..', 'dist', 'src', 'index.js');

const client = new Client({ name: 'e2e-harness', version: '1.0.0' });
await client.connect(
  new StdioClientTransport({ command: process.execPath, args: [gatewayEntry] }),
);

const names = (list) => list.tools.map((t) => t.name).join(', ');

console.log('1. tools before activation:', names(await client.listTools()));

const listed = await client.callTool({ name: 'list_servers', arguments: {} });
console.log('2. list_servers:\n' + listed.content[0].text.split('\n').slice(0, 14).join('\n'));

const activated = await client.callTool({ name: 'activate', arguments: { name: target } });
console.log('3. activate:', activated.content[0].text);

console.log('4. tools after activation:', names(await client.listTools()));

const alias = target.split('.').at(-1);
const called = await client.callTool({ name: `${alias}.echo`, arguments: { text: 'hello' } });
console.log('5. called through the gateway:', JSON.stringify(called.content).slice(0, 200));

const deactivated = await client.callTool({ name: 'deactivate', arguments: { name: target } });
console.log('6. deactivate:', deactivated.content[0].text);
console.log('7. tools after deactivation:', names(await client.listTools()));

await client.close();

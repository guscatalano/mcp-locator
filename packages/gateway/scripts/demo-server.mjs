#!/usr/bin/env node
/**
 * A real (tiny) MCP server, used to demonstrate the full stack end to end.
 * Stands in for "some app on your machine that ships an MCP server".
 */
import { McpServer } from '@modelcontextprotocol/sdk/server/mcp.js';
import { StdioServerTransport } from '@modelcontextprotocol/sdk/server/stdio.js';
import { z } from 'zod';

const server = new McpServer({ name: 'demo-notes', version: '1.0.0' });

server.registerTool(
  'echo',
  {
    description: 'Echo text back, with the pid of the process that handled it.',
    inputSchema: { text: z.string().describe('Text to echo') },
  },
  async ({ text }) => ({
    content: [{ type: 'text', text: `${text} (handled by pid ${process.pid})` }],
  }),
);

server.registerTool(
  'add',
  {
    description: 'Add two numbers.',
    inputSchema: { a: z.number(), b: z.number() },
  },
  async ({ a, b }) => ({ content: [{ type: 'text', text: String(a + b) }] }),
);

await server.connect(new StdioServerTransport());

#!/usr/bin/env node
// Manual smoke test for the broker pipe: connect, send a few requests, print the responses.
//
//   cargo run -p mcp-locator-broker -- serve        # in one terminal
//   node broker/scripts/pipe-smoke.mjs              # in another
//
// The final request asks for a method this build does not implement, so a healthy run ends with
// `method not found` rather than a stubbed grant.
import net from 'node:net';

const defaultAddress =
  process.platform === 'win32'
    ? '\\\\.\\pipe\\mcp-locator\\broker\\v1'
    : `${process.env.XDG_RUNTIME_DIR ?? '/tmp'}/mcp-locator-broker-v1.sock`;

const address = process.argv[2] ?? defaultAddress;

const requests = [
  { jsonrpc: '2.0', id: 1, method: 'locator/handshake', params: { libVersion: '0.1.0', brokerProtocol: 1 } },
  { jsonrpc: '2.0', id: 2, method: 'locator/list', params: { includeOrphaned: true } },
  { jsonrpc: '2.0', id: 3, method: 'locator/status', params: { name: process.argv[3] ?? 'com.example.notes' } },
  { jsonrpc: '2.0', id: 4, method: 'locator/activate', params: { name: 'com.example.notes' } },
];

const socket = net.connect({ path: address });
let buffer = '';
let seen = 0;

socket.on('connect', () => {
  for (const request of requests) socket.write(`${JSON.stringify(request)}\n`);
});

socket.on('data', (chunk) => {
  buffer += chunk.toString();
  let index;
  while ((index = buffer.indexOf('\n')) >= 0) {
    const line = buffer.slice(0, index);
    buffer = buffer.slice(index + 1);
    if (!line.trim()) continue;
    const response = JSON.parse(line);
    console.log(
      `id=${response.id} ${
        response.error
          ? `error ${response.error.code}: ${response.error.message}`
          : JSON.stringify(response.result)
      }`,
    );
    if (++seen === requests.length) socket.end();
  }
});

socket.on('error', (e) => {
  console.error(`connect failed: ${e.message}\n(is the broker running? \`cargo run -p mcp-locator-broker -- serve\`)`);
  process.exit(1);
});

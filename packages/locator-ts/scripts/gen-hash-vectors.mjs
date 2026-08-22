#!/usr/bin/env node
// Regenerates conformance/launch-hash.json from the TypeScript implementation.
// Run only when the vectors are intentionally being changed — the Rust broker is expected to
// match these bytes, so regenerating silently would defeat the point of the cross-check.
import { writeFileSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { canonicalize, launchHash } from '../dist/src/launchHash.js';

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), '..', '..', '..');

const cards = [
  {
    description: 'stdio launch with args',
    card: {
      name: 'com.example.present',
      version: '1.4.2',
      description: 'x',
      local: { launch: { type: 'stdio', command: 'C:\\Program Files\\App\\app.exe', args: ['--serve'] } },
    },
  },
  {
    description: 'executable launch with a named pipe endpoint',
    card: {
      name: 'com.example.pipe',
      version: '1.0.0',
      description: 'x',
      local: {
        launch: { type: 'executable', command: '/usr/bin/app', args: [] },
        endpoint: { type: 'pipe', address: '\\\\.\\pipe\\example' },
      },
    },
  },
  {
    description: 'endpoint-only card, no launch stanza',
    card: {
      name: 'com.example.endpointonly',
      version: '3.1.0',
      description: 'x',
      local: { endpoint: { type: 'streamable-http', address: 'http://127.0.0.1:7000/mcp' } },
    },
  },
  {
    description: 'unicode and escapes in command and env',
    card: {
      name: 'com.example.unicode',
      version: '1.0.0',
      description: 'x',
      local: {
        launch: { type: 'stdio', command: 'C:\\Ünïcodé\\日本語\\app.exe', env: { QUOTE: 'say "hi"\n' } },
      },
    },
  },
  {
    description: 'no local block at all',
    card: { name: 'com.example.remoteonly', version: '1.0.0', description: 'x' },
  },
];

const vectors = cards.map(({ description, card }) => ({
  description,
  card,
  canonical: canonicalize({ launch: card.local?.launch ?? null, endpoint: card.local?.endpoint ?? null }),
  launchHash: launchHash(card),
}));

const out = {
  $comment:
    'Cross-language test vectors for RFC 8785 canonicalization and launchHash (spec/003 section 4). Every implementation must reproduce `canonical` byte-for-byte and `launchHash` exactly.',
  vectors,
};

writeFileSync(join(repoRoot, 'conformance', 'launch-hash.json'), `${JSON.stringify(out, null, 2)}\n`);
console.log(`wrote ${vectors.length} vectors`);

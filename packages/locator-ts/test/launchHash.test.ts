import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { canonicalize, launchHash } from '../src/launchHash.js';
import type { ServerCard } from '../src/types.js';
import { conformanceDir } from './helpers.js';

interface Vector {
  description: string;
  card: ServerCard;
  canonical: string;
  launchHash: string;
}

const vectors = JSON.parse(
  readFileSync(join(conformanceDir, 'launch-hash.json'), 'utf8'),
) as { vectors: Vector[] };

test('cross-language vectors: canonical form and digest are stable', () => {
  // The Rust broker must reproduce these byte-for-byte. Consent binds to launchHash, so any
  // drift between implementations would silently invalidate every stored user decision.
  for (const v of vectors.vectors) {
    assert.equal(
      canonicalize({ launch: v.card.local?.launch ?? null, endpoint: v.card.local?.endpoint ?? null }),
      v.canonical,
      v.description,
    );
    assert.equal(launchHash(v.card), v.launchHash, v.description);
  }
});

test('JCS orders object keys and omits whitespace', () => {
  assert.equal(canonicalize({ b: 1, a: 2 }), '{"a":2,"b":1}');
  assert.equal(canonicalize({ z: [1, 2], a: { d: false, c: null } }), '{"a":{"c":null,"d":false},"z":[1,2]}');
});

test('key order in the source card does not change the hash', () => {
  const one = { name: 'a.b', version: '1', description: 'x', local: { launch: { type: 'stdio' as const, command: 'c', args: ['1'] } } };
  const two = { description: 'x', local: { launch: { args: ['1'], command: 'c', type: 'stdio' as const } }, version: '1', name: 'a.b' };
  assert.equal(launchHash(one), launchHash(two));
});

test('identity-only changes do not invalidate consent', () => {
  const base: ServerCard = { name: 'a.b', version: '1.0.0', description: 'x', local: { launch: { type: 'stdio', command: 'c' } } };
  const bumped: ServerCard = { ...base, version: '2.0.0', title: 'New title' };
  assert.equal(launchHash(base), launchHash(bumped), 'version bumps must not force re-consent');
});

test('changing the launch command does invalidate consent', () => {
  const base: ServerCard = { name: 'a.b', version: '1.0.0', description: 'x', local: { launch: { type: 'stdio', command: 'c' } } };
  const swapped: ServerCard = { ...base, local: { launch: { type: 'stdio', command: 'cmd.exe' } } };
  assert.notEqual(launchHash(base), launchHash(swapped));
});

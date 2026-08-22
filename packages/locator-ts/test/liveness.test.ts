import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { probablyRunning } from '../src/liveness.js';
import type { CatalogEntry, ServerCard } from '../src/types.js';

function entryFor(card: ServerCard): CatalogEntry {
  return { name: card.name, card, raw: card, path: 'x', tier: 'user', orphaned: false, shadowed: [] };
}

function cardWithPidFile(pidFile: string): ServerCard {
  return {
    name: 'com.example.app',
    version: '1.0.0',
    description: 'x',
    local: { launch: { type: 'stdio', command: 'x' }, liveness: { pidFile } },
  };
}

test('a pidfile naming this live process reads as running', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'mcp-locator-live-'));
  const pidFile = join(dir, 'app.pid');
  writeFileSync(pidFile, String(process.pid));
  assert.equal(await probablyRunning(entryFor(cardWithPidFile(pidFile))), true);
});

test('a stale pidfile reads as not running', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'mcp-locator-stale-'));
  const pidFile = join(dir, 'app.pid');
  // PID 0x7FFFFFFE is not a plausible live process on any supported platform.
  writeFileSync(pidFile, '2147483646');
  assert.equal(await probablyRunning(entryFor(cardWithPidFile(pidFile))), false);
});

test('garbage in a pidfile does not throw', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'mcp-locator-junk-'));
  const pidFile = join(dir, 'app.pid');
  writeFileSync(pidFile, 'not-a-pid');
  assert.equal(await probablyRunning(entryFor(cardWithPidFile(pidFile))), false);
});

test('a card with no liveness hints reads as not running', async () => {
  const card: ServerCard = {
    name: 'com.example.app',
    version: '1.0.0',
    description: 'x',
    local: { launch: { type: 'stdio', command: 'x' } },
  };
  assert.equal(await probablyRunning(entryFor(card)), false);
});

test('probing an endpoint nobody is serving fails fast rather than hanging', async () => {
  const card: ServerCard = {
    name: 'com.example.app',
    version: '1.0.0',
    description: 'x',
    local: {
      endpoint: { type: 'streamable-http', address: 'http://127.0.0.1:9' },
      liveness: { probe: true },
    },
  };
  const started = Date.now();
  assert.equal(await probablyRunning(entryFor(card), { timeoutMs: 250 }), false);
  assert.ok(Date.now() - started < 5000, 'probe must respect its timeout');
});

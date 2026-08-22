import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { consentFor, readConsentState } from '../src/consent.js';
import { launchHash } from '../src/launchHash.js';
import type { CatalogEntry, ServerCard } from '../src/types.js';

function entryFor(card: ServerCard): CatalogEntry {
  return { name: card.name, card, raw: card, path: 'x', tier: 'user', orphaned: false, shadowed: [] };
}

const card: ServerCard = {
  name: 'com.example.app',
  version: '1.0.0',
  description: 'x',
  local: { launch: { type: 'stdio', command: 'C:\\app.exe' } },
};

function stateDirWith(store: unknown): string {
  const dir = mkdtempSync(join(tmpdir(), 'mcp-locator-test-'));
  writeFileSync(join(dir, 'consent.json'), JSON.stringify(store));
  return dir;
}

test('absent store reads as not-asked', () => {
  // The consent format ships before its writer does; no broker installed must not look like denial.
  const record = readConsentState('com.example.app', { stateDir: join(tmpdir(), 'mcp-locator-absent') });
  assert.equal(record.state, 'not-asked');
});

test('granted consent survives when the launch stanza is unchanged', () => {
  const stateDir = stateDirWith({
    'com.example.app': { state: 'granted', launchHash: launchHash(card), grantedAt: '2026-01-01T00:00:00Z' },
  });
  assert.equal(consentFor(entryFor(card), { stateDir }).state, 'granted');
});

test('consent goes stale when the launch command changes underneath it', () => {
  const stateDir = stateDirWith({
    'com.example.app': { state: 'granted', launchHash: launchHash(card), grantedAt: '2026-01-01T00:00:00Z' },
  });
  const swapped: ServerCard = { ...card, local: { launch: { type: 'stdio', command: 'C:\\Windows\\cmd.exe' } } };
  assert.equal(consentFor(entryFor(swapped), { stateDir }).state, 'stale');
});

test('denial is not re-interpreted as stale', () => {
  const stateDir = stateDirWith({ 'com.example.app': { state: 'denied' } });
  assert.equal(consentFor(entryFor(card), { stateDir }).state, 'denied');
});

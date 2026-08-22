import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { enumerate, find } from '../src/catalog.js';
import { conformanceDir, fixtureEnv, fixtureRoots, normalize, type NormalizedCatalog } from './helpers.js';

const expected = JSON.parse(
  readFileSync(join(conformanceDir, 'expected', 'basic.json'), 'utf8'),
) as NormalizedCatalog & { visibleByDefault: string[] };

test('conformance/basic: merged catalog matches expected projection', () => {
  const catalog = enumerate({
    roots: fixtureRoots('basic'),
    env: fixtureEnv('basic'),
    includeOrphaned: true,
  });
  const actual = normalize(catalog, 'basic');

  assert.deepEqual(actual.entries, expected.entries);
  assert.deepEqual(actual.diagnostics, expected.diagnostics);
});

test('orphaned cards are hidden unless explicitly requested', () => {
  const catalog = enumerate({ roots: fixtureRoots('basic'), env: fixtureEnv('basic') });
  assert.deepEqual(
    catalog.entries.map((e) => e.name),
    expected.visibleByDefault,
  );
});

test('higher tier shadows lower and records what it shadowed', () => {
  const catalog = enumerate({ roots: fixtureRoots('basic'), env: fixtureEnv('basic') });
  const entry = catalog.entries.find((e) => e.name === 'com.example.shadowed');
  assert.ok(entry);
  assert.equal(entry.tier, 'system');
  assert.equal(entry.card.version, '2.0.0', 'system-tier card must win');
  assert.deepEqual(entry.shadowed.map((s) => s.tier), ['user']);
});

test('a malformed card does not blank out the rest of the catalog', () => {
  const catalog = enumerate({ roots: fixtureRoots('basic'), env: fixtureEnv('basic') });
  assert.ok(catalog.entries.length >= 4);
  assert.ok(catalog.diagnostics.some((d) => d.code === 'malformed-json'));
});

test('env references in launch commands are expanded', () => {
  const entry = find('com.example.present', {
    roots: fixtureRoots('basic'),
    env: fixtureEnv('basic'),
  });
  assert.ok(entry);
  assert.ok(!entry.card.local?.launch?.command.includes('${FIXTURE_ROOT}'));
  assert.ok(entry.raw.local?.launch?.command.includes('${FIXTURE_ROOT}'), 'raw card keeps the reference');
});

test('missing registry directories are not an error', () => {
  const catalog = enumerate({ roots: [{ tier: 'user', path: join(conformanceDir, 'does-not-exist') }] });
  assert.deepEqual(catalog.entries, []);
  assert.deepEqual(catalog.diagnostics, []);
});

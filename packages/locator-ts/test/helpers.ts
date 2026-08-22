import { fileURLToPath } from 'node:url';
import { join, resolve } from 'node:path';
import type { Catalog, Root } from '../src/types.js';

/** Repo root, resolved from the compiled test location (dist/test/…). */
export const repoRoot = resolve(fileURLToPath(new URL('.', import.meta.url)), '..', '..', '..', '..');
export const conformanceDir = join(repoRoot, 'conformance');

export function fixtureRoot(name: string): string {
  return join(conformanceDir, 'fixtures', name);
}

/** Fixture cards reference ${FIXTURE_ROOT}; the harness supplies it. */
export function fixtureEnv(name: string): NodeJS.ProcessEnv {
  return { ...process.env, FIXTURE_ROOT: fixtureRoot(name) };
}

export function fixtureRoots(name: string): Root[] {
  const base = fixtureRoot(name);
  return [
    { tier: 'system', path: join(base, 'system') },
    { tier: 'user', path: join(base, 'user') },
    { tier: 'low', path: join(base, 'low') },
  ];
}

export interface NormalizedCatalog {
  entries: Array<{
    name: string;
    tier: string;
    version: string;
    orphaned: boolean;
    shadowedTiers: string[];
    command: string | null;
  }>;
  diagnostics: Array<{ code: string; name: string | null }>;
}

/**
 * Compare a projection rather than raw paths: absolute paths differ per machine and OS, and
 * every port of this library must agree on the projection, not on local filesystem layout.
 */
export function normalize(catalog: Catalog, fixture: string): NormalizedCatalog {
  const base = fixtureRoot(fixture);
  const strip = (p: string | undefined): string | null =>
    p === undefined ? null : p.replace(base, '').replace(/^[\\/]+/, '').replace(/\\/g, '/');

  return {
    entries: catalog.entries.map((e) => ({
      name: e.name,
      tier: e.tier,
      version: e.card.version,
      orphaned: e.orphaned,
      shadowedTiers: e.shadowed.map((s) => s.tier),
      command: strip(e.card.local?.launch?.command),
    })),
    diagnostics: catalog.diagnostics
      .map((d) => ({ code: d.code as string, name: d.name ?? null }))
      .sort((a, b) => a.code.localeCompare(b.code) || (a.name ?? '').localeCompare(b.name ?? '')),
  };
}

import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import type { CatalogEntry, ConsentRecord } from './types.js';
import { resolveStateDir } from './dirs.js';
import { launchHash } from './launchHash.js';

export interface ConsentOptions {
  /** Override the broker state directory (tests, conformance fixtures). */
  stateDir?: string;
  env?: NodeJS.ProcessEnv;
  platform?: NodeJS.Platform;
}

/**
 * Read-only view of the broker's consent store (spec/003 §4). The file format ships before
 * its writer does: with no broker installed every server reads as `not-asked`.
 */
export function readConsentState(name: string, options: ConsentOptions = {}): ConsentRecord {
  const store = readConsentStore(options);
  return store[name] ?? { state: 'not-asked' };
}

/**
 * Consent as it applies to a specific card. Returns `stale` when the card's launch stanza has
 * changed since approval — the rule that stops an approved card being swapped for something else.
 */
export function consentFor(entry: CatalogEntry, options: ConsentOptions = {}): ConsentRecord {
  const record = readConsentState(entry.name, options);
  if (record.state !== 'granted' || !record.launchHash) return record;
  return record.launchHash === launchHash(entry.card) ? record : { ...record, state: 'stale' };
}

function readConsentStore(options: ConsentOptions): Record<string, ConsentRecord> {
  const env = options.env ?? process.env;
  const dir = options.stateDir ?? resolveStateDir(env, options.platform ?? process.platform);
  try {
    return JSON.parse(readFileSync(join(dir, 'consent.json'), 'utf8')) as Record<string, ConsentRecord>;
  } catch {
    return {}; // absent or unreadable store means nothing has been decided yet
  }
}

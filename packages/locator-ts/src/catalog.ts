import { readdirSync, statSync } from 'node:fs';
import { join } from 'node:path';
import type { Catalog, CatalogEntry, Diagnostic, Root, Tier } from './types.js';
import { TIER_RANK } from './types.js';
import { CARD_SUFFIX, parseCardFile } from './parse.js';
import { resolveCommand } from './command.js';
import { resolveRoots } from './dirs.js';

export interface EnumerateOptions {
  /** Override the registry directories. Used by tests and the conformance suite. */
  roots?: Root[];
  env?: NodeJS.ProcessEnv;
  platform?: NodeJS.Platform;
  /** Include cards whose launch.command is missing from disk. Default false. */
  includeOrphaned?: boolean;
}

/**
 * Merge every registry tier into one catalog. Pure disk reads — no launching, no broker
 * (spec/001 §4). Entries are sorted by name for stable output.
 */
export function enumerate(options: EnumerateOptions = {}): Catalog {
  const env = options.env ?? process.env;
  const roots = options.roots ?? resolveRoots(env, options.platform ?? process.platform);
  const diagnostics: Diagnostic[] = [];

  // Highest-ranked tier wins; ties broken by root order so earlier XDG_DATA_DIRS win.
  const ordered = [...roots].sort((a, b) => TIER_RANK[b.tier] - TIER_RANK[a.tier]);
  const byName = new Map<string, CatalogEntry>();

  for (const root of ordered) {
    for (const file of cardFilesIn(root.path)) {
      const result = parseCardFile(file, root.tier, env);
      diagnostics.push(...result.diagnostics);
      if (!result.card || !result.expanded) continue;

      const name = result.card.name;
      const existing = byName.get(name);
      if (existing) {
        // Same name in a lower tier: record the shadowing rather than merging silently.
        existing.shadowed.push({ path: file, tier: root.tier });
        diagnostics.push({
          code: 'shadowed',
          path: file,
          name,
          message: `shadowed by ${existing.tier} tier card at ${existing.path}`,
        });
        continue;
      }

      byName.set(name, {
        name,
        card: result.expanded,
        raw: result.card,
        path: file,
        tier: root.tier,
        orphaned: isOrphaned(result.expanded, env, options.platform ?? process.platform),
        shadowed: [],
      });
    }
  }

  let entries = [...byName.values()].sort((a, b) => (a.name < b.name ? -1 : a.name > b.name ? 1 : 0));
  if (!options.includeOrphaned) entries = entries.filter((e) => !e.orphaned);

  return { entries, diagnostics };
}

/** A card whose launch binary has vanished — typically a failed uninstall (spec/001 §5). */
function isOrphaned(
  card: { local?: { launch?: { command: string } } },
  env: NodeJS.ProcessEnv,
  platform: NodeJS.Platform,
): boolean {
  const command = card.local?.launch?.command;
  if (!command) return false;
  return resolveCommand(command, env, platform) === undefined;
}

function cardFilesIn(dir: string): string[] {
  let names: string[];
  try {
    if (!statSync(dir).isDirectory()) return [];
    names = readdirSync(dir);
  } catch {
    return []; // missing registry directory is normal, not an error
  }
  return names
    .filter((n) => n.endsWith(CARD_SUFFIX))
    .sort()
    .map((n) => join(dir, n));
}

/** Look up one server by name. Returns undefined when absent or shadowed out. */
export function find(name: string, options: EnumerateOptions = {}): CatalogEntry | undefined {
  return enumerate({ ...options, includeOrphaned: true }).entries.find((e) => e.name === name);
}

export function tierOf(entry: CatalogEntry): Tier {
  return entry.tier;
}

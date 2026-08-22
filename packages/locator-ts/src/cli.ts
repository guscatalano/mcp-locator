#!/usr/bin/env node
import { existsSync } from 'node:fs';
import { enumerate } from './catalog.js';
import { resolveRoots, resolveStateDir } from './dirs.js';
import { parseCardFile } from './parse.js';
import { probablyRunning } from './liveness.js';
import { consentFor } from './consent.js';

const USAGE = `mcp-locator — inspect MCP servers registered on this machine

  mcp-locator ls [--all] [--json]   list registered servers
  mcp-locator validate <card...>    validate card files
  mcp-locator dirs                  show registry and state directories

Read-only. Activation requires the broker (spec/002).`;

async function ls(args: string[]): Promise<number> {
  const includeOrphaned = args.includes('--all');
  const asJson = args.includes('--json');
  const catalog = enumerate({ includeOrphaned });

  const rows = await Promise.all(
    catalog.entries.map(async (entry) => ({
      name: entry.name,
      title: entry.card.title ?? '',
      version: entry.card.version,
      tier: entry.tier,
      running: await probablyRunning(entry),
      consent: consentFor(entry).state,
      orphaned: entry.orphaned,
      path: entry.path,
    })),
  );

  if (asJson) {
    console.log(JSON.stringify({ servers: rows, diagnostics: catalog.diagnostics }, null, 2));
    return 0;
  }

  if (rows.length === 0) {
    console.log('No MCP servers registered.');
    console.log(`Looked in:\n${resolveRoots().map((r) => `  [${r.tier}] ${r.path}`).join('\n')}`);
  } else {
    const width = Math.max(...rows.map((r) => r.name.length));
    for (const row of rows) {
      const state = row.orphaned ? 'orphaned' : row.running ? 'probably-running' : 'registered';
      console.log(
        `${row.name.padEnd(width)}  ${row.version.padEnd(8)} [${row.tier}] ${state}  consent:${row.consent}`,
      );
    }
  }

  for (const d of catalog.diagnostics) {
    console.error(`warning: ${d.code} ${d.path}${d.message ? ` — ${d.message}` : ''}`);
  }
  return 0;
}

function validate(paths: string[]): number {
  if (paths.length === 0) {
    console.error('usage: mcp-locator validate <card...>');
    return 2;
  }
  let failures = 0;
  for (const path of paths) {
    if (!existsSync(path)) {
      console.error(`${path}: not found`);
      failures++;
      continue;
    }
    const result = parseCardFile(path, 'user');
    if (result.diagnostics.length > 0) {
      for (const d of result.diagnostics) console.error(`${path}: ${d.code} — ${d.message}`);
      failures++;
    } else {
      console.log(`${path}: ok (${result.card?.name})`);
    }
  }
  return failures === 0 ? 0 : 1;
}

function dirs(): number {
  console.log('Registry directories (highest tier first):');
  for (const root of resolveRoots()) {
    console.log(`  [${root.tier}] ${root.path}${existsSync(root.path) ? '' : '  (missing)'}`);
  }
  const state = resolveStateDir();
  console.log(`Broker state:\n  ${state}${existsSync(state) ? '' : '  (missing)'}`);
  return 0;
}

const [command, ...args] = process.argv.slice(2);
let code = 0;
switch (command) {
  case 'ls':
    code = await ls(args);
    break;
  case 'validate':
    code = validate(args);
    break;
  case 'dirs':
    code = dirs();
    break;
  case undefined:
  case '-h':
  case '--help':
    console.log(USAGE);
    break;
  default:
    console.error(`unknown command: ${command}\n\n${USAGE}`);
    code = 2;
}
process.exit(code);

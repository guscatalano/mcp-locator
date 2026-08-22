import { readFileSync } from 'node:fs';
import { basename } from 'node:path';
import { createRequire } from 'node:module';
import type { Ajv2020, ErrorObject, Options } from 'ajv/dist/2020.js';
import type { Diagnostic, ServerCard, Tier } from './types.js';
import { expandEnv } from './expand.js';

const require = createRequire(import.meta.url);
const schema = require('@mcp-locator/schema/schemas/v1/local-server-card.schema.json');

// ajv ships CJS; depending on loader the default export is the class or a { default } wrapper,
// so resolve the constructor at runtime rather than relying on ESM interop.
const ajvModule = require('ajv/dist/2020.js') as Record<string, unknown>;
const AjvCtor = (ajvModule['default'] ?? ajvModule['Ajv2020'] ?? ajvModule) as new (opts?: Options) => Ajv2020;
const ajv = new AjvCtor({ allErrors: true, strict: false });
// Deliberately not typed as a type guard: the failure branch still needs to read card.name
// to attribute the diagnostic to a server.
const validateCard = ajv.compile(schema) as ((data: unknown) => boolean) & {
  errors?: ErrorObject[] | null;
};

export const CARD_SUFFIX = '.card.json';

export interface ParseResult {
  card?: ServerCard;
  expanded?: ServerCard;
  diagnostics: Diagnostic[];
}

/**
 * Read and validate one card file. Never throws: every failure becomes a diagnostic so a
 * single bad file cannot blank out the catalog (spec/001 §4).
 */
export function parseCardFile(path: string, _tier: Tier, env: NodeJS.ProcessEnv = process.env): ParseResult {
  let text: string;
  try {
    text = readFileSync(path, 'utf8');
  } catch (e) {
    return { diagnostics: [{ code: 'unreadable', path, message: (e as Error).message }] };
  }

  let card: ServerCard;
  try {
    card = JSON.parse(text) as ServerCard;
  } catch (e) {
    return { diagnostics: [{ code: 'malformed-json', path, message: (e as Error).message }] };
  }

  if (!validateCard(card)) {
    const detail = (validateCard.errors ?? [])
      .map((err) => `${err.instancePath || '/'} ${err.message}`)
      .join('; ');
    return { diagnostics: [{ code: 'schema-invalid', path, message: detail, name: card?.name }] };
  }

  // Filename must match `name` — cheap defense against casual squatting (spec/001 §2).
  const expectedFile = `${card.name}${CARD_SUFFIX}`;
  if (basename(path) !== expectedFile) {
    return {
      diagnostics: [
        {
          code: 'filename-mismatch',
          path,
          name: card.name,
          message: `expected ${expectedFile}, found ${basename(path)}`,
        },
      ],
    };
  }

  return { card, expanded: expandCard(card, env), diagnostics: [] };
}

/** Expand env references in the fields that resolve to filesystem or endpoint locations. */
export function expandCard(card: ServerCard, env: NodeJS.ProcessEnv = process.env): ServerCard {
  const out: ServerCard = structuredClone(card);
  const local = out.local;
  if (!local) return out;

  if (local.launch) {
    local.launch.command = expandEnv(local.launch.command, env);
    if (local.launch.args) local.launch.args = local.launch.args.map((a) => expandEnv(a, env));
    if (local.launch.cwd) local.launch.cwd = expandEnv(local.launch.cwd, env);
    if (local.launch.env) {
      local.launch.env = Object.fromEntries(
        Object.entries(local.launch.env).map(([k, v]) => [k, expandEnv(v, env)]),
      );
    }
  }
  if (local.endpoint) local.endpoint.address = expandEnv(local.endpoint.address, env);
  if (local.liveness?.pidFile) local.liveness.pidFile = expandEnv(local.liveness.pidFile, env);

  return out;
}

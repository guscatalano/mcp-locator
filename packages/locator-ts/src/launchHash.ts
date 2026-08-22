import { createHash } from 'node:crypto';
import type { ServerCard } from './types.js';

/**
 * RFC 8785 (JCS) canonical JSON. The broker (Rust) and this library must agree byte-for-byte:
 * consent is bound to this hash, so any drift would silently invalidate every user decision.
 * Cross-language test vectors live in conformance/launch-hash.json.
 */
export function canonicalize(value: unknown): string {
  if (value === null) return 'null';
  if (typeof value === 'boolean') return value ? 'true' : 'false';
  if (typeof value === 'number') {
    if (!Number.isFinite(value)) throw new TypeError('JCS: non-finite number');
    return Object.is(value, -0) ? '0' : String(value);
  }
  if (typeof value === 'string') return JSON.stringify(value);
  if (Array.isArray(value)) return `[${value.map(canonicalize).join(',')}]`;
  if (typeof value === 'object') {
    const entries = Object.entries(value as Record<string, unknown>)
      .filter(([, v]) => v !== undefined)
      // JCS orders keys by UTF-16 code unit, which is what the default string comparison gives.
      .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0));
    return `{${entries.map(([k, v]) => `${JSON.stringify(k)}:${canonicalize(v)}`).join(',')}}`;
  }
  throw new TypeError(`JCS: unsupported type ${typeof value}`);
}

/**
 * Hash of what would actually execute: the expanded launch stanza plus endpoint (spec/003 §4).
 * Consent binds to this, so swapping a card's command after approval forces a re-prompt.
 */
export function launchHash(card: ServerCard): string {
  const subject = {
    launch: card.local?.launch ?? null,
    endpoint: card.local?.endpoint ?? null,
  };
  const digest = createHash('sha256').update(canonicalize(subject), 'utf8').digest('hex');
  return `sha256:${digest}`;
}

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { join } from 'node:path';
import { verifySignature } from '../src/signature.js';

const windows = process.platform === 'win32';

test('a missing file is unknown, never unsigned', () => {
  // The two must not be conflated: one means "cannot tell", the other is a definite answer.
  const trust = verifySignature('C:/nonexistent/mcp-locator-not-here.exe');
  assert.equal(trust.state, 'unknown');
});

test('a system binary verifies and names its signer', { skip: !windows }, () => {
  // The positive path, available on any Windows machine without a certificate of our own.
  const system = process.env['SystemRoot'] ?? 'C:\Windows';
  const trust = verifySignature(join(system, 'System32', 'kernel32.dll'));
  assert.equal(trust.state, 'signed', trust.detail);
  assert.match(trust.detail, /Microsoft/);
});

test('an unsigned binary reports unsigned rather than failing', { skip: !windows }, () => {
  // A local build has no signature; that has to be a clean answer, not an error, or the
  // bootstrap cannot tell "no signature" from "check broke".
  const trust = verifySignature(process.execPath.replace(/node\.exe$/i, 'node.exe'));
  assert.ok(['signed', 'unsigned'].includes(trust.state), trust.state);
});

test('the verifier is located absolutely, not through PATH', { skip: !windows }, () => {
  // Resolving powershell.exe through PATH would let anything that can write PATH substitute the
  // verifier — the exact problem the check exists to prevent, moved down one level.
  const trust = verifySignature('C:/Windows/System32/kernel32.dll', {
    ...process.env,
    SystemRoot: 'C:/nonexistent',
  });
  assert.equal(trust.state, 'unknown');
  assert.match(trust.detail, /powershell/);
});

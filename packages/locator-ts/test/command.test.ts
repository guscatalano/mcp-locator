import { test } from 'node:test';
import assert from 'node:assert/strict';
import { resolveCommand } from '../src/command.js';

/** A program every machine of this platform has, referred to by bare name. */
const wellKnown = process.platform === 'win32' ? 'cmd' : 'sh';

test('a bare command is resolved through PATH', () => {
  // The case that mattered in practice: a card saying `"command": "node"` is more portable than
  // one naming an absolute path, and used to be hidden as an orphan with no diagnostic.
  const found = resolveCommand(wellKnown, process.env, process.platform);
  assert.ok(found, `${wellKnown} should resolve on PATH`);
});

test('a bare command that is on no PATH does not resolve', () => {
  assert.equal(
    resolveCommand('mcp-locator-definitely-not-installed', process.env, process.platform),
    undefined,
  );
});

test('a command containing a separator is treated as a path, never searched', () => {
  // Otherwise `./tools/notes.exe` missing from the app directory could be silently satisfied by
  // some unrelated `notes.exe` earlier on PATH — the wrong program, launched under a name the
  // user already approved.
  assert.equal(resolveCommand(`./${wellKnown}`, process.env, process.platform), undefined);
});

test('PATH comes from the supplied environment, not the real one', () => {
  const env = { PATH: '', PATHEXT: '.EXE' };
  assert.equal(resolveCommand(wellKnown, env, process.platform), undefined);
});

test('windows resolves a bare name against PATHEXT', () => {
  if (process.platform !== 'win32') return; // PATHEXT is a Windows concept
  // `cmd` only resolves because `.EXE` is appended; without PATHEXT handling every Windows card
  // would have to spell out the extension.
  const withExt = resolveCommand('cmd', { ...process.env, PATHEXT: '.EXE' }, 'win32');
  assert.ok(withExt?.toLowerCase().endsWith('cmd.exe'));
  assert.equal(resolveCommand('cmd', { ...process.env, PATHEXT: '.BAT' }, 'win32'), undefined);
});

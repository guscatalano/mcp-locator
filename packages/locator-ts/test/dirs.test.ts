import { test } from 'node:test';
import assert from 'node:assert/strict';
import { resolveRoots, resolveStateDir } from '../src/dirs.js';

test('windows roots cover system, user, and low integrity', () => {
  const env = {
    ProgramData: 'C:\\ProgramData',
    LOCALAPPDATA: 'C:\\Users\\x\\AppData\\Local',
    USERPROFILE: 'C:\\Users\\x',
  } as NodeJS.ProcessEnv;
  const roots = resolveRoots(env, 'win32');

  assert.deepEqual(roots.map((r) => r.tier), ['system', 'user', 'low']);
  assert.ok(roots[0]!.path.startsWith('C:\\ProgramData'));
  // LocalLow is the one HKCU-equivalent location a low-integrity process can write.
  assert.ok(roots[2]!.path.includes('LocalLow'));
});

test('linux roots honour XDG_DATA_DIRS ordering', () => {
  const env = { HOME: '/home/x', XDG_DATA_DIRS: '/opt/share:/usr/share' } as NodeJS.ProcessEnv;
  const roots = resolveRoots(env, 'linux');

  assert.equal(roots[0]!.path, '/opt/share/mcp-locator/servers');
  assert.equal(roots[1]!.path, '/usr/share/mcp-locator/servers');
  assert.equal(roots.at(-1)!.tier, 'user');
});

test('macos roots are system then user', () => {
  const roots = resolveRoots({ HOME: '/Users/x' } as NodeJS.ProcessEnv, 'darwin');
  assert.deepEqual(roots.map((r) => r.tier), ['system', 'user']);
});

test('state directory is user-scoped on every platform', () => {
  const win = resolveStateDir({ LOCALAPPDATA: 'C:\\l', USERPROFILE: 'C:\\u' } as NodeJS.ProcessEnv, 'win32');
  assert.equal(win, 'C:\\l\\mcp-locator\\state');
  assert.ok(resolveStateDir({ HOME: '/home/x' } as NodeJS.ProcessEnv, 'linux').includes('mcp-locator'));
});

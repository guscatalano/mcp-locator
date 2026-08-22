import { test } from 'node:test';
import assert from 'node:assert/strict';
import { expandEnv } from '../src/expand.js';

const env = { FOO: 'bar', PROGRAMFILES: 'C:\\Program Files' } as NodeJS.ProcessEnv;

test('both %VAR% and ${VAR} expand on every platform', () => {
  assert.equal(expandEnv('%FOO%/x', env), 'bar/x');
  assert.equal(expandEnv('${FOO}/x', env), 'bar/x');
  assert.equal(expandEnv('%PROGRAMFILES%\\app.exe', env), 'C:\\Program Files\\app.exe');
});

test('unknown variables are left verbatim rather than emptied', () => {
  // Emptying would turn a typo into a launch of the wrong path.
  assert.equal(expandEnv('%NOPE%/x', env), '%NOPE%/x');
  assert.equal(expandEnv('${NOPE}/x', env), '${NOPE}/x');
});

test('strings without references are untouched', () => {
  assert.equal(expandEnv('C:\\plain\\path.exe', env), 'C:\\plain\\path.exe');
  assert.equal(expandEnv('50% done', env), '50% done');
});

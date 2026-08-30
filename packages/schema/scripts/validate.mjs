#!/usr/bin/env node
// Validates every *.card.json under the given files/directories against the v1 card schema.
// Usage: node validate.mjs <file-or-dir> [...]
import { readFileSync, readdirSync, statSync, existsSync } from 'node:fs';
import { join, basename } from 'node:path';
import Ajv2020 from 'ajv/dist/2020.js';
import { localServerCardV1 } from '../index.mjs';

const ajv = new Ajv2020.default({ allErrors: true, strict: false });
const validate = ajv.compile(localServerCardV1);

function* cardFiles(path) {
  const st = statSync(path);
  if (st.isDirectory()) {
    for (const entry of readdirSync(path)) yield* cardFiles(join(path, entry));
  } else if (path.endsWith('.card.json')) {
    yield path;
  }
}

let failures = 0;
let count = 0;
for (const root of process.argv.slice(2)) {
  if (!existsSync(root)) continue;
  for (const file of cardFiles(root)) {
    count++;
    let card;
    try {
      // A leading BOM is tolerated here for the same reason the parsers tolerate it:
      // this check has to accept every card they would accept, or CI disagrees with runtime.
      card = JSON.parse(readFileSync(file, 'utf8').replace(/^﻿/, ''));
    } catch (e) {
      // Fixture files exercising the malformed-json diagnostic are expected to fail parsing.
      if (basename(file).startsWith('malformed.')) continue;
      console.error(`PARSE FAIL ${file}: ${e.message}`);
      failures++;
      continue;
    }
    if (!validate(card)) {
      // Fixture files exercising the schema-invalid diagnostic are expected to fail validation.
      if (basename(file).startsWith('invalid.')) continue;
      console.error(`INVALID ${file}:`);
      for (const err of validate.errors ?? []) console.error(`  ${err.instancePath || '/'} ${err.message}`);
      failures++;
    }
  }
}
console.log(`${count} card file(s) checked, ${failures} failure(s)`);
process.exit(failures === 0 ? 0 : 1);

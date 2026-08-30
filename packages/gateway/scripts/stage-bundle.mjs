// Copies the card schema next to the bundle.
//
// The schema package reads its JSON at runtime relative to `import.meta.url`. After bundling,
// that resolves to the bundle's own directory, so the file has to travel with it — a single
// .mjs plus one .json is still a self-contained drop-in, which is what the installer ships.
import { copyFileSync, mkdirSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const rel = 'schemas/v1/local-server-card.schema.json';
const target = join(here, '..', 'dist', 'bundle', rel);

mkdirSync(dirname(target), { recursive: true });
copyFileSync(join(here, '..', '..', 'schema', rel), target);
console.log(`staged ${rel}`);

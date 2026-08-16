import { readFileSync } from 'node:fs';

export const localServerCardV1 = JSON.parse(
  readFileSync(new URL('./schemas/v1/local-server-card.schema.json', import.meta.url), 'utf8'),
);

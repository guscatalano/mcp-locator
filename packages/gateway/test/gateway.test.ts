import { test } from 'node:test';
import assert from 'node:assert/strict';
import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { InMemoryTransport } from '@modelcontextprotocol/sdk/inMemory.js';
import { BrokerError, type BrokerClient } from '@mcp-locator/client';
import { Gateway, aliasFor } from '../src/gateway.js';

/** A broker stand-in: these tests are about gateway behaviour, not the pipe. */
function fakeBroker(overrides: Partial<Record<keyof BrokerClient, unknown>> = {}): BrokerClient {
  return {
    list: async () => [],
    activate: async () => {
      throw new BrokerError(-32000, 'consent required');
    },
    release: async () => ({}),
    deactivate: async () => ({}),
    close: () => {},
    ...overrides,
  } as unknown as BrokerClient;
}

async function connectedClient(gateway: Gateway): Promise<Client> {
  const [clientSide, serverSide] = InMemoryTransport.createLinkedPair();
  const client = new Client({ name: 'test', version: '1.0.0' });
  await Promise.all([client.connect(clientSide), gateway.server.connect(serverSide)]);
  return client;
}

test('meta-tools are always available, before anything is activated', async () => {
  const gateway = new Gateway({ connectBroker: async () => fakeBroker() });
  const client = await connectedClient(gateway);

  const { tools } = await client.listTools();
  assert.deepEqual(
    tools.map((t) => t.name).sort(),
    ['activate', 'deactivate', 'list_servers'],
    'an AI client that has only configured the gateway must still have a way in',
  );
  await client.close();
});

test('list_servers reports state and whether each server can be activated', async () => {
  const gateway = new Gateway({
    connectBroker: async () =>
      fakeBroker({
        list: async () => [
          {
            name: 'com.example.notes',
            version: '1.0.0',
            description: 'notes',
            tier: 'user',
            path: 'x',
            orphaned: false,
            launchHash: 'sha256:a',
            state: 'registered',
            consent: { state: 'granted' },
            grants: 0,
          },
          {
            name: 'com.example.locked',
            version: '1.0.0',
            description: 'locked',
            tier: 'user',
            path: 'y',
            orphaned: false,
            launchHash: 'sha256:b',
            state: 'registered',
            consent: { state: 'not-asked' },
            grants: 0,
          },
        ],
      }),
  });
  const client = await connectedClient(gateway);

  const result = await client.callTool({ name: 'list_servers', arguments: {} });
  const text = (result.content as Array<{ text: string }>)[0]!.text;
  const rows = JSON.parse(text.split('\n\n')[0]!) as Array<{ name: string; activatable: boolean }>;

  assert.equal(rows.find((r) => r.name === 'com.example.notes')?.activatable, true);
  assert.equal(rows.find((r) => r.name === 'com.example.locked')?.activatable, false);
  assert.match(text, /need the user's approval/, 'the model must be told approval is not its call');
  await client.close();
});

test('an unapproved server is reported as needing the user, not as a retryable failure', async () => {
  const gateway = new Gateway({ connectBroker: async () => fakeBroker() });
  const client = await connectedClient(gateway);

  const result = await client.callTool({ name: 'activate', arguments: { name: 'com.example.x' } });

  assert.equal(result.isError, true);
  const text = (result.content as Array<{ text: string }>)[0]!.text;
  assert.match(text, /you cannot approve it yourself/);
  await client.close();
});

test('calling a tool from a server that was never activated explains what to do', async () => {
  const gateway = new Gateway({ connectBroker: async () => fakeBroker() });
  const client = await connectedClient(gateway);

  const result = await client.callTool({ name: 'notes.search', arguments: {} });

  assert.equal(result.isError, true);
  assert.match((result.content as Array<{ text: string }>)[0]!.text, /Activate the server/);
  await client.close();
});

test('deactivating something that is not active is an error, not a silent success', async () => {
  const gateway = new Gateway({ connectBroker: async () => fakeBroker() });
  const client = await connectedClient(gateway);

  const result = await client.callTool({ name: 'deactivate', arguments: { name: 'com.example.x' } });
  assert.equal(result.isError, true);
  await client.close();
});

test('aliases come from the last label and disambiguate without numbers where possible', () => {
  assert.equal(aliasFor('com.example.notes', new Set()), 'notes');
  // Two vendors shipping a "notes" server: qualify with the vendor rather than appending a digit.
  assert.equal(aliasFor('org.other.notes', new Set(['notes'])), 'other_notes');
  assert.equal(aliasFor('notes', new Set(['notes'])), 'notes2');
  assert.equal(aliasFor('com.a.notes', new Set(['notes', 'a_notes'])), 'com_a_notes');
});

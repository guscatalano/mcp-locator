import { test } from 'node:test';
import assert from 'node:assert/strict';
import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { InMemoryTransport } from '@modelcontextprotocol/sdk/inMemory.js';
import { BrokerError, type BrokerClient } from '@mcp-locator/client';
import {
  Gateway,
  aliasFor,
  activationMessage,
  renderServerList,
  selectTools,
  statusOf,
  type ServerRow,
} from '../src/gateway.js';

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

  // `detail: 'full'` because the default listing is now compact lines, not JSON.
  const result = await client.callTool({ name: 'list_servers', arguments: { detail: 'full' } });
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

test('the activation message names the tools without claiming what the client will call them', () => {
  const message = activationMessage('io.example.weather', ['forecast', 'cities']);

  assert.match(message, /forecast, cities/, 'the model still needs to know what arrived');
  // The regression this guards: Hermes renames `weather.forecast` to
  // `mcp__mcp_locator__weather_forecast`, so a model that trusts a prefix we advertise asks for
  // a tool that does not exist. Anything shaped like `alias.tool` here is that bug returning.
  assert.doesNotMatch(
    message,
    /weather\.\*|weather\.forecast/,
    'the gateway must not advertise its wire naming as if the client used it',
  );
  assert.match(message, /your own tool list/, 'it should point at the authority on names');
});

/** A catalogue row, with only the fields a test cares about spelled out. */
function row(name: string, over: Partial<ServerRow> = {}): ServerRow {
  return {
    name,
    title: null,
    description: `what ${name} does`,
    version: '1.0.0',
    trust: 'user',
    state: 'registered',
    consent: 'granted',
    activated: false,
    activatable: true,
    status: 'ready',
    ...over,
  };
}

test('the default listing is one compact line per server, not JSON', () => {
  const text = renderServerList([row('com.example.notes'), row('com.example.weather')]);

  assert.equal(text.split('\n').length, 2, 'one line each, nothing else');
  assert.doesNotMatch(text, /[{}"]/, 'JSON punctuation is most of what makes a catalogue expensive');
  assert.match(text, /com\.example\.notes {2}ready {2}what com\.example\.notes does/);
});

test('a compact line stays cheaper than the detailed form it replaces', () => {
  const rows = Array.from({ length: 100 }, (_, i) => row(`com.example.s${i}`));

  const compact = renderServerList(rows).length;
  const full = renderServerList(rows, { detail: 'full' }).length;

  // The guarantee that matters is a large constant factor, not an exact ratio.
  assert.ok(compact * 3 < full, `compact ${compact} should be far under full ${full}`);
});

test('query narrows the catalogue instead of making the model page through it', () => {
  const text = renderServerList(
    [row('com.example.notes'), row('com.example.weather'), row('io.acme.notepad')],
    { query: 'note' },
  );

  assert.match(text, /com\.example\.notes/);
  assert.match(text, /io\.acme\.notepad/);
  assert.doesNotMatch(text, /weather/);
});

test('a query that matches nothing says so rather than returning an empty page', () => {
  const text = renderServerList([row('com.example.notes')], { query: 'zzz' });

  assert.match(text, /No server matches "zzz"/);
});

test('a long catalogue is paged, and says how to continue', () => {
  const rows = Array.from({ length: 120 }, (_, i) => row(`com.example.s${i}`));

  const first = renderServerList(rows);
  assert.equal(first.split('\n\n')[0]!.split('\n').length, 50, 'default page size');
  assert.match(first, /70 more not shown/);
  assert.match(first, /cursor "50"/);

  const second = renderServerList(rows, { cursor: '50' });
  assert.match(second, /com\.example\.s50/);
  assert.doesNotMatch(second, /com\.example\.s49\b/);
});

test('status reduces the row to the one word the model acts on', () => {
  const granted = { orphaned: false, consent: { state: 'granted' } };
  assert.equal(statusOf(granted, false), 'ready');
  assert.equal(statusOf(granted, true), 'active');
  assert.equal(statusOf({ orphaned: true, consent: { state: 'granted' } }, false), 'unavailable');
  assert.equal(statusOf({ orphaned: false, consent: { state: 'not-asked' } }, false), 'needs-approval');
});

test('activating a subset exposes only the tools that were asked for', () => {
  const all = [{ name: 'forecast' }, { name: 'cities' }, { name: 'radar' }] as Parameters<
    typeof selectTools
  >[1];

  const selected = selectTools('io.example.weather', all, ['forecast']);

  assert.ok('tools' in selected);
  assert.deepEqual(selected.tools.map((t) => t.name), ['forecast']);
});

test('asking for a tool a server does not have is an error, not a silent omission', () => {
  const all = [{ name: 'forecast' }, { name: 'cities' }] as Parameters<typeof selectTools>[1];

  const selected = selectTools('io.example.weather', all, ['forecast', 'tides']);

  // Dropping the unknown name would leave the model believing it had a tool it does not have.
  assert.ok('error' in selected);
  assert.match(selected.error, /does not offer tides/);
  assert.match(selected.error, /It offers: forecast, cities/);
});

test('omitting the filter exposes everything, which stays the default', () => {
  const all = [{ name: 'forecast' }, { name: 'cities' }] as Parameters<typeof selectTools>[1];

  const selected = selectTools('io.example.weather', all);

  assert.ok('tools' in selected);
  assert.equal(selected.tools.length, 2);
});

test('a filtered activation tells the model what it chose not to have', () => {
  const message = activationMessage('io.example.weather', ['forecast'], 2);

  assert.match(message, /2 other tool\(s\) were left out/);
  assert.match(message, /without `tools`/, 'the way back must be in the message');
});

test('activating a hundred-tool server does not spend a kilobyte naming them all', () => {
  const many = Array.from({ length: 100 }, (_, i) => (i === 0 ? 'ping' : `photos_op${i}`));

  const message = activationMessage('com.stress.v07.photos0007', many);

  // Found by the stress run: the message that exists to save context was itself ~1.4KB, and
  // spelling out all hundred names contradicts its own next sentence.
  assert.ok(message.length < 400, `message was ${message.length}B`);
  assert.match(message, /Its 100 tool\(s\)/, 'the count is the useful part');
  assert.match(message, /ping, photos_op1/, 'a sample still shows what arrived');
  assert.match(message, /and 88 more/);
  assert.doesNotMatch(message, /photos_op99/, 'the tail belongs in the tool list, not here');
});

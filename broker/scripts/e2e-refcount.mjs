// End-to-end: two independent clients share one refcounted server through the broker.
import net from 'node:net';

const PIPE = '\\\\.\\pipe\\mcp-locator\\broker\\v1';
const NAME = 'com.example.echo';

function client(label) {
  const socket = net.connect({ path: PIPE });
  let buffer = '';
  const pending = new Map();
  let nextId = 1;

  socket.on('data', (chunk) => {
    buffer += chunk.toString();
    let i;
    while ((i = buffer.indexOf('\n')) >= 0) {
      const line = buffer.slice(0, i);
      buffer = buffer.slice(i + 1);
      if (!line.trim()) continue;
      const msg = JSON.parse(line);
      pending.get(msg.id)?.(msg);
      pending.delete(msg.id);
    }
  });

  const ready = new Promise((res, rej) => {
    socket.once('connect', res);
    socket.once('error', rej);
  });

  return {
    label,
    ready,
    call(method, params) {
      const id = nextId++;
      return new Promise((resolve) => {
        pending.set(id, resolve);
        socket.write(`${JSON.stringify({ jsonrpc: '2.0', id, method, params })}\n`);
      });
    },
    kill() {
      socket.destroy();
    },
  };
}

async function talkToServer(address, text) {
  const s = net.connect({ path: address });
  await new Promise((res, rej) => {
    s.once('connect', res);
    s.once('error', rej);
  });
  s.write(`${JSON.stringify({ jsonrpc: '2.0', id: 99, method: text })}\n`);
  const line = await new Promise((res) => s.once('data', (d) => res(d.toString())));
  s.destroy();
  return JSON.parse(line);
}

const status = async (c) => (await c.call('locator/status', { name: NAME })).result;
const log = (...a) => console.log(...a);

const a = client('A');
const b = client('B');
await Promise.all([a.ready, b.ready]);

log('1. before activation:', JSON.stringify(await status(a)));

const grantA = (await a.call('locator/activate', { name: NAME })).result;
log(`2. A activated -> grant ${grantA.grantId} at ${grantA.connection.address}`);
log('   status:', JSON.stringify(await status(a)));

const echo = await talkToServer(grantA.connection.address, 'hello-from-A');
log(`3. A talked to the real server, which answered from pid ${echo.result.pid}`);

const grantB = (await b.call('locator/activate', { name: NAME })).result;
log(`4. B activated -> grant ${grantB.grantId}`);
log('   status:', JSON.stringify(await status(a)));

const refused = await a.call('locator/deactivate', { name: NAME });
log(`5. deactivate refused: ${refused.error.message}`);

log('6. killing client A without releasing (simulating a crash)');
a.kill();
await new Promise((r) => setTimeout(r, 300));
log('   status:', JSON.stringify(await status(b)));

await b.call('locator/release', { grantId: grantB.grantId });
log('7. B released its grant');
log('   status:', JSON.stringify(await status(b)));

b.kill();

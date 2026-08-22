import net from 'node:net';
import { spawn } from 'node:child_process';
import { existsSync } from 'node:fs';
import { setTimeout as delay } from 'node:timers/promises';
import type { ConsentRecord, ServerCard, Tier } from './types.js';
import { enumerate } from './catalog.js';
import { resolveRoots } from './dirs.js';

/** Protocol version this client speaks (spec/002 §2). */
export const BROKER_PROTOCOL = 1;

/** Card id of the broker itself — the bootstrap entry every client knows how to find. */
export const BROKER_CARD = 'io.mcplocator.broker';

export type ServerState = 'registered' | 'launching' | 'running' | 'idle' | 'stopping' | 'orphaned';

export interface BrokerServer {
  name: string;
  version: string;
  title?: string;
  description: string;
  tier: Tier;
  path: string;
  orphaned: boolean;
  launchHash: string;
  state: ServerState;
  consent: ConsentRecord;
  grants: number;
}

export interface Grant {
  grantId: string;
  connection: { type: string; address: string };
}

export class BrokerError extends Error {
  constructor(
    readonly code: number,
    message: string,
  ) {
    super(message);
    this.name = 'BrokerError';
  }

  /** The broker refused because the user has not approved this server (spec/003 §4). */
  get isConsentRequired(): boolean {
    return this.code === -32000;
  }

  /** Another client is holding a grant; deactivating anyway needs `force`. */
  get isInUse(): boolean {
    return this.code === -32002;
  }
}

export interface BrokerClientOptions {
  /** Override the broker address. Defaults to the platform pipe/socket. */
  address?: string;
  /** Start the broker if it is not already listening. Default true. */
  autostart?: boolean;
  env?: NodeJS.ProcessEnv;
  platform?: NodeJS.Platform;
}

export function defaultBrokerAddress(
  env: NodeJS.ProcessEnv = process.env,
  platform: NodeJS.Platform = process.platform,
): string {
  if (platform === 'win32') return '\\\\.\\pipe\\mcp-locator\\broker\\v1';
  const runtime = env['XDG_RUNTIME_DIR'] ?? '/tmp';
  return `${runtime}/mcp-locator-broker-v1.sock`;
}

/**
 * Client for the broker's JSON-RPC pipe (spec/002 §3).
 *
 * Everything with side effects lives here; plain catalog reads work without a broker at all
 * (see `enumerate`). Grants are tied to this connection: dropping it releases every server this
 * client activated, which is what makes a crashed client harmless.
 */
export class BrokerClient {
  #socket?: net.Socket;
  #buffer = '';
  #nextId = 1;
  #pending = new Map<number, { resolve: (v: unknown) => void; reject: (e: Error) => void }>();
  #closed = false;

  private constructor(readonly address: string) {}

  /** Connect, starting the broker first if it is not listening. */
  static async connect(options: BrokerClientOptions = {}): Promise<BrokerClient> {
    const env = options.env ?? process.env;
    const platform = options.platform ?? process.platform;
    const address = options.address ?? defaultBrokerAddress(env, platform);
    const client = new BrokerClient(address);

    try {
      await client.#open();
    } catch (e) {
      if (options.autostart === false) throw e;
      await startBroker({ env, platform });
      await client.#open(20);
    }

    const hello = (await client.call('locator/handshake', {
      libVersion: '0.1.0',
      brokerProtocol: BROKER_PROTOCOL,
    })) as { brokerProtocol: number; brokerVersion: string };

    if (hello.brokerProtocol !== BROKER_PROTOCOL) {
      client.close();
      throw new BrokerError(
        -32600,
        `broker speaks protocol ${hello.brokerProtocol}, this client speaks ${BROKER_PROTOCOL}`,
      );
    }
    return client;
  }

  async #open(attempts = 1): Promise<void> {
    let lastError: Error | undefined;
    for (let i = 0; i < attempts; i++) {
      try {
        this.#socket = await connectSocket(this.address);
        this.#attach(this.#socket);
        return;
      } catch (e) {
        lastError = e as Error;
        await delay(100);
      }
    }
    throw lastError ?? new Error(`could not connect to broker at ${this.address}`);
  }

  #attach(socket: net.Socket): void {
    socket.on('data', (chunk) => {
      this.#buffer += chunk.toString();
      let index: number;
      while ((index = this.#buffer.indexOf('\n')) >= 0) {
        const line = this.#buffer.slice(0, index);
        this.#buffer = this.#buffer.slice(index + 1);
        if (line.trim()) this.#handle(line);
      }
    });
    // A broker that goes away mid-call must fail those calls rather than hang them forever.
    socket.on('close', () => this.#failAll(new Error('broker connection closed')));
    socket.on('error', (e) => this.#failAll(e));
  }

  #handle(line: string): void {
    let message: { id?: number; result?: unknown; error?: { code: number; message: string } };
    try {
      message = JSON.parse(line);
    } catch {
      return; // a malformed frame is the broker's bug; dropping it beats crashing the client
    }
    if (typeof message.id !== 'number') return;
    const pending = this.#pending.get(message.id);
    if (!pending) return;
    this.#pending.delete(message.id);

    if (message.error) pending.reject(new BrokerError(message.error.code, message.error.message));
    else pending.resolve(message.result);
  }

  #failAll(error: Error): void {
    this.#closed = true;
    for (const pending of this.#pending.values()) pending.reject(error);
    this.#pending.clear();
  }

  call(method: string, params?: unknown): Promise<unknown> {
    if (this.#closed || !this.#socket) {
      return Promise.reject(new Error('broker connection is closed'));
    }
    const id = this.#nextId++;
    return new Promise((resolve, reject) => {
      this.#pending.set(id, { resolve, reject });
      this.#socket!.write(`${JSON.stringify({ jsonrpc: '2.0', id, method, params })}\n`);
    });
  }

  async list(includeOrphaned = false): Promise<BrokerServer[]> {
    const result = (await this.call('locator/list', { includeOrphaned })) as {
      servers: BrokerServer[];
    };
    return result.servers;
  }

  status(name: string): Promise<{
    name: string;
    state: ServerState;
    grants: number;
    holders: Array<number | null>;
    consent: ConsentRecord;
  }> {
    return this.call('locator/status', { name }) as Promise<{
      name: string;
      state: ServerState;
      grants: number;
      holders: Array<number | null>;
      consent: ConsentRecord;
    }>;
  }

  /** Start (or join) a server. Throws `BrokerError` with `isConsentRequired` when unapproved. */
  activate(name: string): Promise<Grant> {
    return this.call('locator/activate', { name }) as Promise<Grant>;
  }

  release(grantId: string): Promise<unknown> {
    return this.call('locator/release', { grantId });
  }

  /** Stop a server outright. Without `force` this fails when other clients hold grants. */
  deactivate(name: string, force = false): Promise<unknown> {
    return this.call('locator/deactivate', { name, force });
  }

  consent(name: string): Promise<ConsentRecord> {
    return this.call('locator/consent/query', { name }) as Promise<ConsentRecord>;
  }

  close(): void {
    this.#closed = true;
    this.#socket?.destroy();
  }
}

function connectSocket(address: string): Promise<net.Socket> {
  return new Promise((resolve, reject) => {
    const socket = net.connect({ path: address });
    socket.once('connect', () => resolve(socket));
    socket.once('error', (e) => reject(e));
  });
}

/**
 * Launch the broker.
 *
 * Hardened per spec/003 §3: the broker card is honoured **only** from the system tier, and its
 * command must resolve inside the broker's install root. Both checks matter — path containment
 * alone is defeated by a writable subdirectory, and honouring a user-tier card would let any
 * process that can drop a file have its binary started by every AI client on the machine.
 *
 * Signature verification is still to come; until then this refuses rather than pretending.
 */
export async function startBroker(
  options: { env?: NodeJS.ProcessEnv; platform?: NodeJS.Platform } = {},
): Promise<void> {
  const env = options.env ?? process.env;
  const platform = options.platform ?? process.platform;

  const systemRoots = resolveRoots(env, platform).filter((r) => r.tier === 'system');
  const entry = enumerate({ roots: systemRoots, env, platform, includeOrphaned: true }).entries.find(
    (e) => e.name === BROKER_CARD,
  );

  if (!entry) {
    throw new Error(
      `no broker registered in the system tier (looked for ${BROKER_CARD} in ${systemRoots
        .map((r) => r.path)
        .join(', ')})`,
    );
  }

  const command = entry.card.local?.launch?.command;
  if (!command || !existsSync(command)) {
    throw new Error(`broker card names a command that does not exist: ${command}`);
  }
  if (!withinInstallRoot(command, env, platform)) {
    throw new Error(`refusing to launch a broker outside its install root: ${command}`);
  }

  const child = spawn(command, entry.card.local?.launch?.args ?? [], {
    detached: true,
    stdio: 'ignore',
  });
  child.unref();
}

function withinInstallRoot(
  command: string,
  env: NodeJS.ProcessEnv,
  platform: NodeJS.Platform,
): boolean {
  const roots =
    platform === 'win32'
      ? [`${env['ProgramFiles'] ?? 'C:\\Program Files'}\\mcp-locator`]
      : ['/usr/local/lib/mcp-locator', '/usr/lib/mcp-locator', '/opt/mcp-locator'];

  const normalized = command.replace(/\//g, '\\').toLowerCase();
  return roots.some((root) => normalized.startsWith(root.replace(/\//g, '\\').toLowerCase()));
}

export type { ServerCard };

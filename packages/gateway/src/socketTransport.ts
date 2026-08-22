import net from 'node:net';
import type { Transport } from '@modelcontextprotocol/sdk/shared/transport.js';
import type { JSONRPCMessage } from '@modelcontextprotocol/sdk/types.js';

/**
 * MCP transport over a named pipe or unix socket.
 *
 * The broker hands out a per-grant relay address rather than a child process, so the SDK's
 * stdio transport does not apply — but the framing is identical: one JSON-RPC message per line.
 */
export class SocketClientTransport implements Transport {
  #socket?: net.Socket;
  #buffer = '';

  onmessage?: (message: JSONRPCMessage) => void;
  onerror?: (error: Error) => void;
  onclose?: () => void;

  constructor(
    private readonly address: string,
    private readonly connectTimeoutMs = 5000,
  ) {}

  async start(): Promise<void> {
    this.#socket = await this.#connect();

    this.#socket.on('data', (chunk) => {
      this.#buffer += chunk.toString();
      let index: number;
      while ((index = this.#buffer.indexOf('\n')) >= 0) {
        const line = this.#buffer.slice(0, index);
        this.#buffer = this.#buffer.slice(index + 1);
        if (!line.trim()) continue;
        try {
          this.onmessage?.(JSON.parse(line) as JSONRPCMessage);
        } catch (e) {
          this.onerror?.(e as Error);
        }
      }
    });
    this.#socket.on('error', (e) => this.onerror?.(e));
    this.#socket.on('close', () => this.onclose?.());
  }

  /**
   * The broker returns the relay address before it has accepted on it, so a first connect can
   * legitimately race ahead of the listener. Retry briefly rather than failing the activation.
   */
  #connect(): Promise<net.Socket> {
    const deadline = Date.now() + this.connectTimeoutMs;

    const attempt = (): Promise<net.Socket> =>
      new Promise((resolve, reject) => {
        const socket = net.connect({ path: this.address });
        socket.once('connect', () => resolve(socket));
        socket.once('error', (e) => reject(e));
      }).catch(async (e) => {
        if (Date.now() >= deadline) throw e;
        await new Promise((r) => setTimeout(r, 25));
        return attempt();
      }) as Promise<net.Socket>;

    return attempt();
  }

  async send(message: JSONRPCMessage): Promise<void> {
    if (!this.#socket) throw new Error('transport is not started');
    this.#socket.write(`${JSON.stringify(message)}\n`);
  }

  async close(): Promise<void> {
    this.#socket?.destroy();
    this.#socket = undefined;
  }
}

import { readFileSync } from 'node:fs';
import net from 'node:net';
import type { CatalogEntry } from './types.js';

const DEFAULT_PROBE_TIMEOUT_MS = 300;

export interface LivenessOptions {
  timeoutMs?: number;
}

/**
 * Best-effort liveness. The name is the contract: without the broker this can be stale (a
 * pidfile outlives a crash) or racy (a probe can hit a half-open socket). Callers that need
 * the truth ask the broker for `status` instead (spec/001 §4, spec/002 §3).
 */
export async function probablyRunning(entry: CatalogEntry, options: LivenessOptions = {}): Promise<boolean> {
  const liveness = entry.card.local?.liveness;

  if (liveness?.pidFile && pidFileAlive(liveness.pidFile)) return true;

  const endpoint = entry.card.local?.endpoint;
  if (liveness?.probe && endpoint) {
    return probeEndpoint(endpoint.type, endpoint.address, options.timeoutMs ?? DEFAULT_PROBE_TIMEOUT_MS);
  }

  return false;
}

function pidFileAlive(pidFile: string): boolean {
  try {
    const pid = Number.parseInt(readFileSync(pidFile, 'utf8').trim(), 10);
    if (!Number.isInteger(pid) || pid <= 0) return false;
    process.kill(pid, 0);
    return true;
  } catch (e) {
    // EPERM means the process exists but belongs to someone else — still alive.
    return (e as NodeJS.ErrnoException).code === 'EPERM';
  }
}

function probeEndpoint(type: string, address: string, timeoutMs: number): Promise<boolean> {
  return new Promise((resolve) => {
    let socket: net.Socket;
    const done = (result: boolean) => {
      socket?.destroy();
      resolve(result);
    };

    try {
      if (type === 'streamable-http') {
        const url = new URL(address);
        const port = Number.parseInt(url.port, 10) || (url.protocol === 'https:' ? 443 : 80);
        socket = net.connect({ host: url.hostname, port });
      } else {
        // Named pipes and unix sockets both connect by path in Node.
        socket = net.connect({ path: address });
      }
    } catch {
      resolve(false);
      return;
    }

    socket.setTimeout(timeoutMs);
    socket.once('connect', () => done(true));
    socket.once('timeout', () => done(false));
    socket.once('error', () => done(false));
  });
}

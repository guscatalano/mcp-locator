import { Client } from '@modelcontextprotocol/sdk/client/index.js';
// The low-level `Server` is marked deprecated in favour of `McpServer`, but its own note keeps
// it for advanced use — and this is one. `McpServer.registerTool` takes Zod shapes, whereas a
// gateway must pass upstream tools through with their original JSON Schemas untouched.
// eslint-disable-next-line @typescript-eslint/no-deprecated
import { Server } from '@modelcontextprotocol/sdk/server/index.js';
import {
  CallToolRequestSchema,
  ListToolsRequestSchema,
  type Tool,
} from '@modelcontextprotocol/sdk/types.js';
import { BrokerClient, BrokerError, type BrokerServer } from '@mcp-locator/client';
import { SocketClientTransport } from './socketTransport.js';

/** Separates the server's short name from the tool's own name: `notes.search`. */
const NAMESPACE_SEPARATOR = '.';

interface ActiveServer {
  name: string;
  grantId: string;
  client: Client;
  tools: Tool[];
  /** Short name used to namespace this server's tools. */
  alias: string;
}

const META_TOOLS: Tool[] = [
  {
    name: 'list_servers',
    description:
      'List the MCP servers registered on this machine, with their state and whether the user has approved them. Call this first to discover what is available.',
    inputSchema: {
      type: 'object',
      properties: {
        includeOrphaned: {
          type: 'boolean',
          description: 'Include servers whose program is no longer installed. Defaults to false.',
        },
      },
    },
  },
  {
    name: 'activate',
    description:
      "Start a registered MCP server and expose its tools. The server's tools become callable as `<alias>.<tool>` immediately after this returns. Requires that the user has already approved the server.",
    inputSchema: {
      type: 'object',
      properties: { name: { type: 'string', description: 'Server name, e.g. com.example.notes' } },
      required: ['name'],
    },
  },
  {
    name: 'deactivate',
    description:
      'Stop using a server activated earlier and remove its tools. The server shuts down once no other client is using it.',
    inputSchema: {
      type: 'object',
      properties: { name: { type: 'string', description: 'Server name to release' } },
      required: ['name'],
    },
  },
];

/**
 * One MCP server standing in front of every server the broker knows about.
 *
 * This is the adoption path: an AI client configures the gateway once, and everything registered
 * on the machine afterwards shows up without touching that client's config again. Tools appear
 * mid-session — activation triggers `notifications/tools/list_changed`, which is what lets the
 * model discover and then use a server within one conversation.
 */
export interface GatewayOptions {
  autostartBroker?: boolean;
  /** Override how the broker connection is obtained. Used by tests to inject a stub. */
  connectBroker?: () => Promise<BrokerClient>;
}

export class Gateway {
  readonly server: Server;
  #broker?: BrokerClient;
  #active = new Map<string, ActiveServer>();

  constructor(private readonly options: GatewayOptions = {}) {
    this.server = new Server(
      { name: 'mcp-locator', version: '0.1.0' },
      { capabilities: { tools: { listChanged: true } } },
    );

    this.server.setRequestHandler(ListToolsRequestSchema, async () => ({
      tools: [...META_TOOLS, ...this.#exportedTools()],
    }));

    this.server.setRequestHandler(CallToolRequestSchema, async (request) => {
      const { name, arguments: args } = request.params;
      switch (name) {
        case 'list_servers':
          return this.#listServers(Boolean((args as { includeOrphaned?: boolean })?.includeOrphaned));
        case 'activate':
          return this.#activate(String((args as { name?: string })?.name ?? ''));
        case 'deactivate':
          return this.#deactivate(String((args as { name?: string })?.name ?? ''));
        default:
          return this.#forward(name, args);
      }
    });
  }

  async #brokerClient(): Promise<BrokerClient> {
    if (!this.#broker) {
      this.#broker = this.options.connectBroker
        ? await this.options.connectBroker()
        : await BrokerClient.connect({ autostart: this.options.autostartBroker });
    }
    return this.#broker;
  }

  #exportedTools(): Tool[] {
    return [...this.#active.values()].flatMap((server) =>
      server.tools.map((tool) => ({
        ...tool,
        name: `${server.alias}${NAMESPACE_SEPARATOR}${tool.name}`,
        description: `[${server.name}] ${tool.description ?? ''}`.trim(),
      })),
    );
  }

  async #listServers(includeOrphaned: boolean) {
    const broker = await this.#brokerClient();
    const servers = await broker.list(includeOrphaned);

    const rows = servers.map((server: BrokerServer) => ({
      name: server.name,
      title: server.title,
      description: server.description,
      version: server.version,
      trust: server.tier,
      state: server.state,
      consent: server.consent.state,
      activated: this.#active.has(server.name),
      // Being explicit here keeps the model from trying to activate something that will refuse.
      activatable:
        server.consent.state === 'granted' && !server.orphaned && !this.#active.has(server.name),
    }));

    const unapproved = rows.filter((r) => r.consent !== 'granted').length;
    const hint = unapproved
      ? `\n\n${unapproved} server(s) need the user's approval before they can be activated; the user grants that outside this conversation.`
      : '';

    return {
      content: [{ type: 'text' as const, text: JSON.stringify(rows, null, 2) + hint }],
    };
  }

  async #activate(name: string) {
    if (!name) return errorResult('activate requires a server name');
    if (this.#active.has(name)) return textResult(`${name} is already active.`);

    const broker = await this.#brokerClient();

    let grant;
    try {
      grant = await broker.activate(name);
    } catch (e) {
      if (e instanceof BrokerError && e.isConsentRequired) {
        // Distinguish "the user has not approved this" from a real failure: the model should
        // report it and stop, not retry.
        return errorResult(
          `${name} has not been approved by the user, so it cannot be started. Ask the user to approve it; you cannot approve it yourself.`,
        );
      }
      return errorResult(`could not activate ${name}: ${(e as Error).message}`);
    }

    const client = new Client({ name: 'mcp-locator-gateway', version: '0.1.0' });
    try {
      await client.connect(new SocketClientTransport(grant.connection.address));
      const { tools } = await client.listTools();
      const alias = aliasFor(name, new Set([...this.#active.values()].map((s) => s.alias)));
      this.#active.set(name, { name, grantId: grant.grantId, client, tools, alias });

      // The whole point of the gateway: tools appear inside the running session.
      await this.server.sendToolListChanged();

      return textResult(
        `Activated ${name}. ${tools.length} tool(s) now available as ${alias}${NAMESPACE_SEPARATOR}*: ${tools
          .map((t) => t.name)
          .join(', ')}`,
      );
    } catch (e) {
      // Never leave a grant dangling because the MCP handshake failed after the broker
      // already started the server.
      await client.close().catch(() => {});
      await broker.release(grant.grantId).catch(() => {});
      return errorResult(`${name} started but did not speak MCP: ${(e as Error).message}`);
    }
  }

  async #deactivate(name: string) {
    const active = this.#active.get(name);
    if (!active) return errorResult(`${name} is not active`);

    this.#active.delete(name);
    await active.client.close().catch(() => {});
    await (await this.#brokerClient()).release(active.grantId).catch(() => {});
    await this.server.sendToolListChanged();

    return textResult(`Deactivated ${name}. Its tools are no longer available.`);
  }

  async #forward(namespaced: string, args: unknown) {
    const separator = namespaced.indexOf(NAMESPACE_SEPARATOR);
    const alias = separator > 0 ? namespaced.slice(0, separator) : '';
    const toolName = separator > 0 ? namespaced.slice(separator + 1) : namespaced;
    const server = [...this.#active.values()].find((s) => s.alias === alias);

    if (!server) {
      return errorResult(
        `unknown tool: ${namespaced}. Activate the server that provides it first, or call list_servers to see what is available.`,
      );
    }

    try {
      return await server.client.callTool({
        name: toolName,
        arguments: (args ?? {}) as Record<string, unknown>,
      });
    } catch (e) {
      return errorResult(`${server.name} failed to run ${toolName}: ${(e as Error).message}`);
    }
  }

  /** Release every grant this gateway holds. The broker would reclaim them on disconnect too. */
  async shutdown(): Promise<void> {
    for (const active of this.#active.values()) {
      await active.client.close().catch(() => {});
    }
    this.#active.clear();
    this.#broker?.close();
    this.#broker = undefined;
  }
}

/** `com.example.notes` → `notes`, disambiguated if that is already taken. */
export function aliasFor(name: string, taken: Set<string>): string {
  const labels = name.split('.').filter(Boolean);
  const base = labels.at(-1) ?? name;
  if (!taken.has(base)) return base;

  // Walk back up the reverse-DNS name for a qualifier before resorting to numbers.
  for (let i = labels.length - 2; i >= 0; i--) {
    const candidate = labels.slice(i).join('_');
    if (!taken.has(candidate)) return candidate;
  }
  let n = 2;
  while (taken.has(`${base}${n}`)) n++;
  return `${base}${n}`;
}

function textResult(text: string) {
  return { content: [{ type: 'text' as const, text }] };
}

function errorResult(text: string) {
  return { content: [{ type: 'text' as const, text }], isError: true };
}

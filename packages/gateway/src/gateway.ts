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
import { BROKER_CARD, BrokerClient, BrokerError, type BrokerServer } from '@mcp-locator/client';
import { SocketClientTransport } from './socketTransport.js';

/** Separates the server's short name from the tool's own name: `notes.search`. */
const NAMESPACE_SEPARATOR = '.';

/**
 * How many servers one `list_servers` call returns by default.
 *
 * Discovery is the one call a model always makes, so its cost is paid every session. A machine
 * with a thousand registered servers would otherwise answer the first question with a hundred
 * thousand tokens of catalogue — more context than the tools it exists to keep out.
 */
const DEFAULT_LIMIT = 50;

/** Descriptions are advertising copy; a couple of lines is enough to choose by. */
const COMPACT_DESCRIPTION_CHARS = 110;

/** How many tool names an activation message spells out before summarising the rest. */
const NAMED_ON_ACTIVATION = 12;

interface ActiveServer {
  name: string;
  grantId: string;
  client: Client;
  /** Exposed to the model. A subset of `allTools` when `activate` was given a filter. */
  tools: Tool[];
  /** Everything the server offers, kept so a call to a filtered-out tool can explain itself. */
  allTools: Tool[];
  /** Short name used to namespace this server's tools. */
  alias: string;
}

const META_TOOLS: Tool[] = [
  {
    name: 'list_servers',
    description:
      'List the MCP servers registered on this machine, with their state and whether the user has approved them. Call this first to discover what is available. Narrow a long list with `query` rather than paging through all of it.',
    inputSchema: {
      type: 'object',
      properties: {
        query: {
          type: 'string',
          description:
            'Only list servers whose name or description contains this text. Case-insensitive.',
        },
        detail: {
          type: 'string',
          enum: ['compact', 'full'],
          description:
            'compact (default) gives one line per server: name, status, short description. full adds version, trust tier and consent state as JSON.',
        },
        limit: {
          type: 'number',
          description: `Maximum servers to return. Defaults to ${DEFAULT_LIMIT}.`,
        },
        cursor: {
          type: 'string',
          description: 'Continue a previous listing, using the cursor it returned.',
        },
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
      'Start a registered MCP server and expose its tools, which become callable immediately after this returns. Requires that the user has already approved the server. Pass `tools` to expose only the ones you need from a server that offers many.',
    inputSchema: {
      type: 'object',
      properties: {
        name: { type: 'string', description: 'Server name, e.g. com.example.notes' },
        tools: {
          type: 'array',
          items: { type: 'string' },
          description:
            "Expose only these tools, named as the server itself names them (no prefix). Omit to expose all of them. Use this when a server offers many tools and you only need a few — every exposed tool's schema costs context.",
        },
      },
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
          return this.#listServers((args ?? {}) as ListOptions);
        case 'activate':
          return this.#activate(
            String((args as { name?: string })?.name ?? ''),
            (args as { tools?: unknown })?.tools,
          );
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

  async #listServers(options: ListOptions) {
    const broker = await this.#brokerClient();
    const servers = await broker.list(Boolean(options.includeOrphaned));

    const rows = servers
      // The broker registers itself as a card so client libraries can find and start it. It is
      // not an MCP server, so offering it here would put a tool in front of the model whose
      // only possible outcome is a failed handshake.
      .filter((server: BrokerServer) => server.name !== BROKER_CARD)
      .map((server: BrokerServer) => ({
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
        status: statusOf(server, this.#active.has(server.name)),
      }));

    return textResult(renderServerList(rows, options));
  }

  async #activate(name: string, requested?: unknown) {
    if (!name) return errorResult('activate requires a server name');
    if (this.#active.has(name)) return textResult(`${name} is already active.`);

    let wanted: string[] | undefined;
    if (requested !== undefined) {
      if (!Array.isArray(requested) || requested.some((t) => typeof t !== 'string')) {
        return errorResult('activate: `tools` must be an array of tool names');
      }
      wanted = requested as string[];
      if (wanted.length === 0) {
        return errorResult(
          'activate: `tools` was empty. Omit it to expose every tool, or name the ones you need.',
        );
      }
    }

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
      const { tools: allTools } = await client.listTools();

      const selected = selectTools(name, allTools, wanted);
      if ('error' in selected) {
        // Fail rather than quietly exposing a subset the model did not ask for: it would go on
        // to call a tool that is not there, which is the failure this whole change is about.
        await client.close().catch(() => {});
        await broker.release(grant.grantId).catch(() => {});
        return errorResult(selected.error);
      }
      const tools = selected.tools;

      const alias = aliasFor(name, new Set([...this.#active.values()].map((s) => s.alias)));
      this.#active.set(name, { name, grantId: grant.grantId, client, tools, allTools, alias });

      // The whole point of the gateway: tools appear inside the running session.
      await this.server.sendToolListChanged();

      return textResult(
        activationMessage(
          name,
          tools.map((t) => t.name),
          allTools.length - tools.length,
        ),
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

    // A tool held back by an `activate` filter is not callable. Forwarding it anyway would make
    // the filter advisory, and a model that guessed a name would get a different answer from a
    // model that read the tool list.
    if (!server.tools.some((t) => t.name === toolName)) {
      const withheld = server.allTools.some((t) => t.name === toolName);
      return errorResult(
        withheld
          ? `${toolName} was not exposed when ${server.name} was activated. Deactivate it and activate again, either without a tools filter or including ${toolName}.`
          : `${server.name} has no tool called ${toolName}. It exposes: ${server.tools
              .map((t) => t.name)
              .join(', ')}.`,
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

/**
 * What `activate` reports back to the model.
 *
 * It names the tools but deliberately never claims what the *client* will call them.
 * `alias.tool` is this gateway's wire naming, and a client is free to rewrite it: Hermes
 * presents `weather.forecast` to its model as `mcp__mcp_locator__weather_forecast`. An earlier
 * version of this message advertised the wire name, a model believed it, asked for a tool that
 * did not exist under that name, and spent two round trips recovering. The `tools/list_changed`
 * notification already tells the client to re-read, and its own list is the only authority.
 */
export function activationMessage(name: string, toolNames: string[], withheld = 0): string {
  // Naming every tool of a hundred-tool server costs about 1.4KB and contradicts the very next
  // sentence, which tells the model to read its own tool list. A sample is enough to recognise
  // what arrived; the list itself is the authority, as it says.
  const shown = toolNames.slice(0, NAMED_ON_ACTIVATION);
  const rest = toolNames.length - shown.length;
  const listed = rest > 0 ? `${shown.join(', ')}, and ${rest} more` : shown.join(', ');

  const held = withheld
    ? ` ${withheld} other tool(s) were left out because you asked for a subset; activate again without \`tools\` to get them all.`
    : '';
  return (
    `Activated ${name}. Its ${toolNames.length} tool(s) are now in your tool list: ` +
    `${listed}.${held} Your client decides how these are named, so take the exact ` +
    `names from your own tool list rather than assuming a prefix.`
  );
}

/**
 * Narrow a server's tools to the ones `activate` asked for.
 *
 * An unknown name is an error rather than a silent omission. Dropping it would leave the model
 * believing it had a tool it does not have — the same failure as advertising a name the client
 * never registered, arrived at from the other direction.
 */
export function selectTools(
  server: string,
  allTools: Tool[],
  wanted?: string[],
): { tools: Tool[] } | { error: string } {
  if (!wanted) return { tools: allTools };

  const offered = allTools.map((t) => t.name);
  const unknown = wanted.filter((t) => !offered.includes(t));
  if (unknown.length) {
    return {
      error: `${server} does not offer ${unknown.join(', ')}. It offers: ${offered.join(', ')}.`,
    };
  }
  return { tools: allTools.filter((t) => wanted.includes(t.name)) };
}

/** Options accepted by `list_servers`. */
export interface ListOptions {
  query?: string;
  detail?: string;
  limit?: number;
  cursor?: string;
  includeOrphaned?: boolean;
}

/** The shape `list_servers` works with — the broker's row plus what this gateway knows. */
export interface ServerRow {
  name: string;
  title?: string | null;
  description: string;
  version: string;
  trust: string;
  state: string;
  consent: string;
  activated: boolean;
  activatable: boolean;
  status: string;
}

/** One word for what the model can do with this server right now. */
export function statusOf(
  server: { orphaned: boolean; consent: { state: string } },
  activated: boolean,
): string {
  if (activated) return 'active';
  if (server.orphaned) return 'unavailable';
  if (server.consent.state !== 'granted') return 'needs-approval';
  return 'ready';
}

function clip(text: string, max: number): string {
  const flat = text.replace(/\s+/g, ' ').trim();
  if (flat.length <= max) return flat;
  const cut = flat.slice(0, max);
  const boundary = cut.lastIndexOf(' ');
  return `${cut.slice(0, boundary > max * 0.6 ? boundary : max).trimEnd()}…`;
}

/**
 * Render a catalogue for the model, narrowed and paged.
 *
 * Compact is the default because this is the one call every session makes. A full row costs
 * roughly 280 bytes once pretty-printed; a compact line costs about 60, and the pretty-printing
 * alone — newlines and two-space indents on nine fields — was a fifth of the payload. Detail is
 * available on request, which is the right way round: pay for it when a choice needs it.
 */
export function renderServerList(rows: ServerRow[], options: ListOptions = {}): string {
  const query = options.query?.trim().toLowerCase();
  const matched = query
    ? rows.filter((r) =>
        `${r.name} ${r.title ?? ''} ${r.description}`.toLowerCase().includes(query),
      )
    : rows;

  const offset = Number.parseInt(options.cursor ?? '0', 10);
  const start = Number.isFinite(offset) && offset > 0 ? offset : 0;
  const limit =
    typeof options.limit === 'number' && options.limit > 0 ? Math.floor(options.limit) : DEFAULT_LIMIT;
  const page = matched.slice(start, start + limit);

  const body =
    options.detail === 'full'
      ? JSON.stringify(page)
      : page.map((r) => `${r.name}  ${r.status}  ${clip(r.description, COMPACT_DESCRIPTION_CHARS)}`).join('\n');

  const notes: string[] = [];
  if (!matched.length) {
    notes.push(
      query
        ? `No server matches "${options.query}". Call again without a query to see everything.`
        : 'No servers are registered on this machine.',
    );
  }
  const remaining = matched.length - (start + page.length);
  if (remaining > 0) {
    notes.push(
      `${remaining} more not shown. Narrow with \`query\`, or pass cursor "${start + page.length}" to continue.`,
    );
  }
  const unapproved = page.filter((r) => r.consent !== 'granted').length;
  if (unapproved) {
    notes.push(
      `${unapproved} server(s) need the user's approval before they can be activated; the user grants that outside this conversation.`,
    );
  }

  return [body, notes.join(' ')].filter(Boolean).join('\n\n');
}

function textResult(text: string) {
  return { content: [{ type: 'text' as const, text }] };
}

function errorResult(text: string) {
  return { content: [{ type: 'text' as const, text }], isError: true };
}

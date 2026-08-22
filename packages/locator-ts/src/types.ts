/** Trust tier a card was found in. Provenance, not permission — see spec/003 §2. */
export type Tier = 'package' | 'system' | 'user' | 'low';

/** Higher rank shadows lower when the same `name` appears in multiple tiers. */
export const TIER_RANK: Record<Tier, number> = { package: 3, system: 2, user: 1, low: 0 };

export interface LaunchStanza {
  type: 'stdio' | 'executable';
  command: string;
  args?: string[];
  cwd?: string | null;
  env?: Record<string, string>;
  /** Allow one server process to serve concurrent grants. Broker-enforced; default false. */
  shared?: boolean;
}

export interface EndpointStanza {
  type: 'pipe' | 'unix-socket' | 'streamable-http';
  address: string;
}

export interface LivenessStanza {
  pidFile?: string;
  probe?: boolean;
}

export interface LifetimeStanza {
  idleTimeoutSeconds?: number;
  shutdown?: 'graceful' | 'kill';
}

export interface LocalBlock {
  launch?: LaunchStanza;
  endpoint?: EndpointStanza;
  liveness?: LivenessStanza;
  lifetime?: LifetimeStanza;
  consent?: { summary?: string };
}

export interface RemoteStanza {
  type: 'streamable-http' | 'sse';
  url: string;
  [k: string]: unknown;
}

export interface ServerCard {
  $schema?: string;
  name: string;
  version: string;
  description: string;
  title?: string;
  websiteUrl?: string;
  icons?: Array<{ src: string; mimeType?: string; sizes?: string; theme?: 'light' | 'dark' }>;
  repository?: { url?: string; source?: string; subfolder?: string };
  remotes?: RemoteStanza[];
  local?: LocalBlock;
  _meta?: Record<string, unknown>;
}

export interface CatalogEntry {
  name: string;
  /** Card with environment variables expanded — what would actually run. */
  card: ServerCard;
  /** Card exactly as read from disk. */
  raw: ServerCard;
  path: string;
  tier: Tier;
  /**
   * launch.command does not exist on disk. Uninstallers are unreliable, so orphans are
   * hidden from default listings rather than trusted (spec/001 §5).
   */
  orphaned: boolean;
  /** Same-name cards in lower tiers that this entry shadows. */
  shadowed: Array<{ path: string; tier: Tier }>;
}

export type DiagnosticCode =
  | 'malformed-json'
  | 'schema-invalid'
  | 'filename-mismatch'
  | 'unreadable'
  | 'shadowed';

export interface Diagnostic {
  code: DiagnosticCode;
  path: string;
  message: string;
  name?: string;
}

export interface Catalog {
  entries: CatalogEntry[];
  diagnostics: Diagnostic[];
}

export type ConsentState = 'granted' | 'denied' | 'not-asked' | 'stale';

export interface ConsentRecord {
  state: ConsentState;
  grantedAt?: string;
  launchHash?: string;
  scope?: 'user' | 'client';
}

export interface Root {
  tier: Tier;
  path: string;
}

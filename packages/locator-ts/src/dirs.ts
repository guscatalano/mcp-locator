import { homedir } from 'node:os';
import path from 'node:path';
import type { Root } from './types.js';

const APP = 'mcp-locator';
const SERVERS = 'servers';

/**
 * Path semantics must follow the *target* platform, not the host: these functions take a
 * platform argument, so joining with the host flavour would emit backslashes in Linux paths.
 */
function joinerFor(platform: NodeJS.Platform): path.PlatformPath['join'] {
  return platform === 'win32' ? path.win32.join : path.posix.join;
}

/**
 * Registry directories in tier order (spec/001 §2). Missing directories are returned
 * anyway — callers skip what does not exist, and watchers need the paths regardless.
 */
export function resolveRoots(
  env: NodeJS.ProcessEnv = process.env,
  platform: NodeJS.Platform = process.platform,
): Root[] {
  const join = joinerFor(platform);
  const home = env['HOME'] ?? env['USERPROFILE'] ?? homedir();

  if (platform === 'win32') {
    const programData = env['ProgramData'] ?? 'C:\\ProgramData';
    const localAppData = env['LOCALAPPDATA'] ?? join(home, 'AppData', 'Local');
    const localLow = join(home, 'AppData', 'LocalLow');
    return [
      { tier: 'system', path: join(programData, APP, SERVERS) },
      { tier: 'user', path: join(localAppData, APP, SERVERS) },
      { tier: 'low', path: join(localLow, APP, SERVERS) },
    ];
  }

  if (platform === 'darwin') {
    return [
      { tier: 'system', path: join('/Library', 'Application Support', APP, SERVERS) },
      { tier: 'user', path: join(home, 'Library', 'Application Support', APP, SERVERS) },
    ];
  }

  const xdgDataHome = env['XDG_DATA_HOME'] ?? join(home, '.local', 'share');
  const xdgDataDirs = (env['XDG_DATA_DIRS'] ?? '/usr/local/share:/usr/share').split(':').filter(Boolean);
  return [
    ...xdgDataDirs.map((d): Root => ({ tier: 'system', path: join(d, APP, SERVERS) })),
    { tier: 'user', path: join(xdgDataHome, APP, SERVERS) },
  ];
}

/** State directory the broker owns (spec/002 §5). Read-only for this library. */
export function resolveStateDir(
  env: NodeJS.ProcessEnv = process.env,
  platform: NodeJS.Platform = process.platform,
): string {
  const join = joinerFor(platform);
  const home = env['HOME'] ?? env['USERPROFILE'] ?? homedir();

  if (platform === 'win32') {
    const localAppData = env['LOCALAPPDATA'] ?? join(home, 'AppData', 'Local');
    return join(localAppData, APP, 'state');
  }
  if (platform === 'darwin') return join(home, 'Library', 'Application Support', APP, 'state');
  return join(env['XDG_STATE_HOME'] ?? join(home, '.local', 'state'), APP);
}

import { existsSync } from 'node:fs';
import path from 'node:path';

/**
 * Resolve a card's `launch.command` to a file on disk, searching PATH for a bare name.
 *
 * A card that says `"command": "node"` is more portable than one that hard-codes
 * `C:/Program Files/nodejs/node.exe`, because the interpreter lives somewhere different on
 * every machine. Treating a bare name as missing made those cards vanish from the catalog with
 * no diagnostic at all — they were classed as orphaned and hidden, which reads as "the app is
 * not installed" rather than "look somewhere else for this program".
 *
 * The rule matches what the OS will actually do when the broker launches it: a command with a
 * separator in it is a path, and anything else is a name to look up. Getting that wrong in
 * either direction is worse than not checking — reporting a runnable server as orphaned hides
 * it, and reporting a missing one as fine turns a clear message into a launch failure later.
 *
 * `env` rather than `process.env` so the conformance fixtures can pin PATH and both language
 * implementations can be driven from the same inputs.
 */
export function resolveCommand(
  command: string,
  env: NodeJS.ProcessEnv,
  platform: NodeJS.Platform,
): string | undefined {
  const p = platform === 'win32' ? path.win32 : path.posix;
  const separators = platform === 'win32' ? ['\\', '/'] : ['/'];

  if (separators.some((s) => command.includes(s))) {
    return existsSync(command) ? command : undefined;
  }

  // Windows resolves a bare name against PATHEXT, so `node` finds `node.exe`. Elsewhere the
  // name is used as written.
  const extensions =
    platform === 'win32'
      ? ['', ...(env['PATHEXT'] ?? '.COM;.EXE;.BAT;.CMD').split(';').filter(Boolean)]
      : [''];

  const delimiter = platform === 'win32' ? ';' : ':';
  for (const dir of (env['PATH'] ?? env['Path'] ?? '').split(delimiter)) {
    if (!dir) continue;
    for (const extension of extensions) {
      const candidate = p.join(dir, command + extension);
      if (existsSync(candidate)) return candidate;
    }
  }
  return undefined;
}

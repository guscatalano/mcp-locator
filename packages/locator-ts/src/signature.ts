import { execFileSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import { join } from 'node:path';

export type TrustState = 'signed' | 'unsigned' | 'invalid' | 'unknown';

export interface Trust {
  state: TrustState;
  /** Leaf certificate subject when signed; a short reason otherwise. */
  detail: string;
}

/**
 * Authenticode status of a file, for the broker-launch decision in spec/003 §3.
 *
 * The obvious implementation — shell out to the broker's own `verify` subcommand — is circular:
 * asking a binary whether it can be trusted is a question a malicious one answers "yes". So this
 * uses PowerShell's `Get-AuthenticodeSignature`, invoked by absolute path out of System32, which
 * makes the verifier an OS component rather than the thing under test. Resolving it through PATH
 * would reintroduce the same problem one level down, since PATH is writable by the user.
 *
 * Everything that is not a clean `Valid` is a failure, and the states are kept apart because
 * they mean different things: `unsigned` is the normal state of a local build, while `invalid`
 * means a file changed after someone signed it.
 */
export function verifySignature(path: string, env: NodeJS.ProcessEnv = process.env): Trust {
  if (process.platform !== 'win32') {
    return { state: 'unknown', detail: 'no signature verification on this platform yet' };
  }
  if (!existsSync(path)) {
    return { state: 'unknown', detail: 'file not found' };
  }

  const systemRoot = env['SystemRoot'] ?? 'C:\\Windows';
  const powershell = join(systemRoot, 'System32', 'WindowsPowerShell', 'v1.0', 'powershell.exe');
  if (!existsSync(powershell)) {
    return { state: 'unknown', detail: 'powershell.exe not found in System32' };
  }

  let raw: string;
  try {
    raw = execFileSync(
      powershell,
      [
        '-NoProfile',
        '-NonInteractive',
        '-Command',
        // The path travels in the environment, never interpolated into the script text. With
        // -Command, PowerShell parses everything it is given as script, so a filename spliced
        // in there would be code — and filenames are attacker-controlled here, since anyone who
        // can write a card chooses one. -LiteralPath then stops wildcards being expanded.
        '$s = Get-AuthenticodeSignature -LiteralPath $env:MCP_LOCATOR_VERIFY_PATH; ' +
          '"$($s.Status)`n$($s.SignerCertificate.Subject)"',
      ],
      {
        encoding: 'utf8',
        timeout: 20_000,
        windowsHide: true,
        env: {
          ...env,
          MCP_LOCATOR_VERIFY_PATH: path,
          // Pin the module path to the in-box modules. Two reasons, and the second is the
          // important one: an inherited PSModulePath can be mangled (a value set by a POSIX
          // shell breaks module loading outright, which reads as "cannot verify"), and a
          // PSModulePath under someone else's control could shadow Get-AuthenticodeSignature
          // with their own — replacing the verifier is a far cheaper attack than defeating it.
          PSModulePath: join(systemRoot, 'System32', 'WindowsPowerShell', 'v1.0', 'Modules'),
        },
      },
    );
  } catch (e) {
    return { state: 'unknown', detail: (e as Error).message };
  }

  const [status = '', subject = ''] = raw.trim().split(/\r?\n/);
  switch (status.trim()) {
    case 'Valid':
      return { state: 'signed', detail: commonName(subject) || subject.trim() };
    case 'NotSigned':
      return { state: 'unsigned', detail: 'unsigned' };
    case 'HashMismatch':
      return { state: 'invalid', detail: 'the file was modified after it was signed' };
    case 'UnknownError':
    case 'NotTrusted':
      return { state: 'invalid', detail: 'the signing certificate is not trusted on this machine' };
    default:
      return { state: 'invalid', detail: status.trim() || 'unrecognised status' };
  }
}

/** `CN=Contoso Ltd, O=…` reads better as `Contoso Ltd`. */
function commonName(subject: string): string {
  return /CN=([^,]+)/.exec(subject)?.[1]?.trim() ?? '';
}

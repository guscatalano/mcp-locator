/**
 * Expand environment references in card strings. Both `%VAR%` and `${VAR}` are accepted on
 * every platform so a single card file stays portable (spec/001 §3). Unknown variables are
 * left verbatim — silently emptying a path would turn a typo into a wrong-file launch.
 */
export function expandEnv(value: string, env: NodeJS.ProcessEnv = process.env): string {
  return value
    .replace(/%([A-Za-z_][A-Za-z0-9_]*)%/g, (whole, name: string) => env[name] ?? whole)
    .replace(/\$\{([A-Za-z_][A-Za-z0-9_]*)\}/g, (whole, name: string) => env[name] ?? whole);
}

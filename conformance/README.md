# Conformance fixtures

The spec in executable form. Every implementation of the card registry — the TypeScript client
library today, the Rust broker in M2, any later port — runs these same fixtures and must agree.
Where the prose in spec/001 and a fixture disagree, that is a bug in one of them; fix both.

## Layout

- `fixtures/basic/{system,user,low}/` — a synthetic three-tier registry exercising shadowing,
  tier precedence, orphan detection, env expansion, and every diagnostic code.
- `fixtures/basic/bin/present.exe` — a stub file that exists on disk, so `orphaned` detection has
  something real to find. Never executed.
- `expected/basic.json` — the normalized catalog the fixture must produce.
- `launch-hash.json` — cross-language test vectors for RFC 8785 canonicalization and
  `launchHash`. Rust and TypeScript must produce identical digests or consent silently breaks
  (spec/003 §4).

## Normalization

Expected output compares a projection, not raw paths — absolute paths differ per machine and OS.
Each entry is reduced to `{ name, tier, version, orphaned, shadowedTiers, command }`, and
diagnostics to `{ code, name }` sorted by code then name.

`${FIXTURE_ROOT}` in fixture cards is expanded by the test harness to the absolute path of
`fixtures/basic`, which is also what makes the orphan/present distinction testable cross-platform.

# Implementation Plan

Language split (decided): **Rust** for the broker + consent helper, **TypeScript** for the client
library and gateway shim. Contracts between components are the card files and the JSON-RPC pipe
protocol — languages never share code, they share fixtures.

Platform order: library is cross-platform from day one; the broker is **Windows-first**, with the
pipe/launch/signature layers behind traits so the launchd/systemd ports (post-M3) are additive.

## Repo layout (monorepo)

```
packages/schema/          JSON Schema for local-server-card v1 + versioned changelog
packages/locator-ts/      client library (npm: @mcp-locator/client) + CLI
packages/gateway/         M3 gateway shim (npm: @mcp-locator/gateway)
broker/                   Rust cargo workspace
  crates/broker/          daemon
  crates/consent-ui/      consent helper (Win32 TaskDialog first)
  crates/proto/           JSON-RPC types, launchHash canonicalization
conformance/              language-neutral fixtures + expected outputs (used by TS now, ports later)
spec/                     normative docs (already committed)
```

## M0 — Infrastructure (small, do immediately)

- npm workspace + cargo workspace, MIT license, `.gitattributes` (eol normalization — first
  commit already warned about CRLF), rustfmt/clippy/eslint/prettier config.
  (Substituted npm for pnpm: corepack could not install pnpm shims without admin rights, and
  npm workspaces cover everything this repo needs.)
- CI (GitHub Actions): windows-latest + ubuntu + macos matrix. Jobs: TS build/test, Rust
  build/test (Windows only until port), schema-validate all `examples/` and `conformance/` cards.

## M1 — Card schema + read-only TypeScript library

Goal: `npm install @mcp-locator/client` is useful on its own, on all three OSes.

1. **Schema** (`packages/schema`): JSON Schema draft 2020-12 for the card, including the SEP-2127
   identity subset and the `local` block from spec/001 §3. CI validates every example/fixture.
2. **Directory + parse layer**: tier path resolution per OS, card parsing with ajv, filename↔name
   enforcement, env-var expansion, tier shadowing + conflict reporting, orphan detection
   (launch.command missing ⇒ hidden by default), malformed cards surfaced as diagnostics not
   throws.
3. **Liveness**: `probablyRunning()` — pidfile (exists + PID alive) and/or endpoint probe
   (named pipe / unix socket / loopback HTTP connect with short timeout).
4. **Watching**: directory watchers → `onCatalogChanged` events, debounced. (Built on `node:fs`
   watch rather than chokidar: this library gets embedded in AI clients, so its dependency
   surface stays at the schema validator alone.)
5. **Consent read view**: parse `consent.json` per spec/003 §4; returns `not-asked` for all when
   the file is absent (broker doesn't exist yet — the file format ships before the writer).
6. **Conformance suite** (`conformance/`): fixture registry trees + expected merged-catalog JSON
   (shadowing, conflicts, orphans, malformed, env expansion, low tier). This is the spec's
   executable form; every future port runs the same fixtures.
7. **CLI** (`mcp-locator`): `ls` (catalog + state), `validate <card>`, `dirs`. Dogfooding and the
   support tool for app developers writing their first card.

Exit: v0.1 published; all conformance fixtures pass on the 3-OS CI matrix.
Rough size: 1–2 weeks.

## M2 — Broker (Rust, Windows) — the core milestone

Build order chosen so each step is testable against the previous:

1. **Pipe server + protocol** (`crates/proto`, `crates/broker`) — *done, except as noted*:
   newline-delimited JSON-RPC over named pipe `\\.\pipe\mcp-locator\broker\v1` (unix socket
   elsewhere, so all three CI platforms build), `handshake`, `list`, `status`. Still open:
   the SDDL ACL (user SID + low-IL connect ACE) and `subscribe`.
2. **Derived catalog** — *done, except file-watching*: card parsing, tier shadowing, orphan
   detection, and env expansion, driven by the same conformance fixtures as the TS library and
   asserted against the same expected JSON. The notify-crate watcher is still to come.
   Card validation is hand-written against the spec rules rather than embedding a JSON Schema
   engine (which would dominate the broker's dependency tree); the fixtures are what keep the
   two rule sets from drifting.
3. **Activation engine** — *done*: stdio children spawned inside a job object (broker death ⇒
   no orphans), per-grant relay pipe `\\.\pipe\mcp-locator\conn\<grantId>`, grants table, and
   client-death cleanup via connection close rather than process-handle waiting (see spec/002 §3
   for why). One child per grant for stdio; refcounted sharing for endpoint-mode servers.
   Still open: ACLing each relay pipe to the requesting client's token.
4. **Lifetime state machine** — *mostly done*: idle timers, graceful shutdown (close stdin →
   grace → kill), `deactivate`/`force` naming its holders, and the registered/running/idle
   transitions. Still open: server-crash detection and the `runtime.json` snapshot +
   reconcile-on-start.
5. **Consent** — *done*: `consent.json` writer with atomic replace, `launchHash` binding with
   stale detection, enforcement at activation, and the Win32 TaskDialog helper
   (`crates/consent-ui`) showing the server, its launch command, the card's origin and tier, and
   the requesting process — the last read from the PID rather than self-reported. Activation
   raises it; the answer comes back as an exit code, so nothing a client sends reaches the
   dialog. Stale approvals re-prompt with the old and new commands side by side, which is why
   the record now stores `launchCommand` next to the hash. Prompts are serialized machine-wide
   and low-integrity clients cannot raise one at all.
   Still open: allow-for-this-client scope (`ConsentScope::Client` exists but nothing writes it).
6. **Bootstrap hardening** — *partly done*: the MSI puts the broker under `%ProgramFiles%` and
   the client library refuses to launch one from anywhere else, so path containment is real and
   enforced. Still open: the singleton named mutex, `WinVerifyTrust` verification used by both
   the TS library and `admin/supersede`, drain/handover, and the `MCP_LOCATOR_DEV=1` escape
   hatch that goes with them. Signature checking is the one gate that needs a certificate
   bought before it can mean anything (see Risks).
7. **TS library integration**: broker client in `@mcp-locator/client` — connect, launch-on-demand
   bootstrap, `activate`/`release`/`deactivate`, subscriptions; single public API that degrades
   from broker to brokerless transparently (`status` vs `probablyRunning` stay distinct).
8. **Audit log** (append-only JSON lines) + `odr`-style CLI additions: `mcp-locator activate/
   deactivate/status` against the live broker.
9. **Installer** — *done*: WiX v5 MSI (`installer/`) installing the broker, consent helper, and
   the bundled gateway to `%ProgramFiles%\mcp-locator\`, generating and placing the broker's
   system-tier card, and applying the spec/003 §6 permissions. Those are applied by
   `mcp-locator-broker secure-dirs` rather than declared in the MSI, so the same rule holds
   however a machine was set up; the per-user half runs on every `serve`. CI builds the MSI on
   windows-latest and uploads it. Unsigned until there is a certificate.

Exit (the demo that proves the model) — *met*: two separate client processes activate `com.example.notes`;
refcount holds one server; killing client A releases its grant; client B keeps working; closing B
starts the idle timer; server exits gracefully; consent was prompted exactly once; audit log shows
all of it.
Rough size: 4–6 weeks. Steps 1–4 are the critical path; 5–6 are security-gated before any public
release; 9 can trail.

## M3 — Gateway shim (TypeScript) — *done*

1. ✔ MCP server (official TS SDK, stdio) exposing `list_servers` / `activate` / `deactivate`.
   Uses the low-level `Server`, not `McpServer`: a gateway must pass upstream tools through with
   their original JSON Schemas, and `registerTool` takes Zod shapes.
2. ✔ On activate: MCP client session over the granted relay (custom socket transport, since the
   broker hands out an address rather than a child process), tools re-exported as
   `<alias>.<tool>`, `notifications/tools/list_changed` emitted. The broker's own card is
   filtered out of `list_servers`: it is registered so libraries can start it, not so a model
   can try to speak MCP to it. Resources and prompts still to mirror.
3. ✔ Release-on-exit, plus release-on-failed-handshake so a server that starts but does not
   speak MCP cannot leave a dangling grant.
4. ✔ Broker client in `@mcp-locator/client` (`BrokerClient`), with the spec/003 §3 bootstrap
   rules enforced: system-tier card only, command must resolve inside the install root.
   Signature verification still to come.
5. ✔ Onboarding docs in the README and INSTALL.md. Still open: a demo recording with a real
   AI client.

The gateway ships as a single bundled `.mjs` (esbuild) so the MSI can install it without a
`node_modules` tree. That required the client library's two `createRequire` calls to become
static imports — a bundler cannot follow a runtime `require`, and the failure would have been a
gateway that worked from a checkout and not from an install.

Exit criterion met end to end: a real MCP client sees only the three meta-tools, discovers the
demo server, activates it, watches `notes.echo` / `notes.add` appear mid-session, calls one and
reaches the child process, then deactivates and watches them disappear.

## M4 — Federation + upstream (incremental, in value order)

1. **MSIX `appExtension` provider** (highest local value: the `package` trust tier).
2. **Broker port** to macOS/Linux (unix sockets, codesign verification, launchd/systemd units).
3. **Remote catalogs**: SEP-2127 `.well-known` fetcher; **mDNS** `_mcp._tcp` listener (separate
   consent class per spec/003).
4. **Windows ODR provider**: wrap `odr.exe` / the host API; revisit when ODR reaches GA.
5. **Upstream**: propose "local server cards" as an extensions-track SEP — the spec/001 diff
   against SEP-2127 plus the running implementation.

## Cross-cutting

- **Versioning**: schema version in `$schema` URL; `brokerProtocol` integer; additive-only within
  v1. Conformance fixtures are tagged by schema version.
- **Testing pyramid**: conformance fixtures (shared) → Rust integration tests spawning real
  processes → one end-to-end smoke on Windows CI (broker + TS client + fake server).
- **Cross-target lint before pushing.** Half the broker is behind `cfg(windows)`, so a
  Windows-only `cargo clippy` cannot see the other half — code reachable only from the Windows
  module reads as dead on unix. Catch it locally with
  `cargo clippy --all-targets --target x86_64-unknown-linux-gnu -- -D warnings`
  (`rustup target add` once; no linker needed, since clippy does not link).
- **Security gates**: spec/003 checklist review before the first binary release; fuzz the card
  parser and the pipe framing (cheap, high value — both parse attacker-controlled input).

## Risks

| Risk | Mitigation |
|---|---|
| No Authenticode cert early on | `MCP_LOCATOR_DEV=1` mode; buy OV/EV cert before first public MSI; Sigstore as interim for non-Windows |
| Stdio relay latency/backpressure bugs | keep relay dumb (single duplex copy loop); e2e test with large payloads early in M2.3 |
| launchHash canonicalization drift between Rust and TS | shared test vectors in `conformance/`; canonicalization spec'd byte-exact (JCS / RFC 8785) |
| Consent UI spoofing concerns | helper is signed, launched only by broker, renders on interactive desktop; document in spec/003 |
| ODR ships GA sooner than expected | architecture already treats it as a provider; no bet depends on it staying preview |

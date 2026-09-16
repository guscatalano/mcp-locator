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
   elsewhere, so all three CI platforms build), `handshake`, `list`, `status`. The SDDL ACL is
   now applied to both pipes (`crates/broker/src/security.rs`), with a deliberate asymmetry: the
   broker pipe is labelled Low so a sandboxed client can still discover, while each relay pipe is
   labelled at the requesting client's own integrity level so nothing below it can hijack a
   grant. Still open: `subscribe`.
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
   Each relay pipe is ACL'd to the requesting client's token (see step 1).
4. **Lifetime state machine** — *mostly done*: idle timers, graceful shutdown (close stdin →
   grace → kill), `deactivate`/`force` naming its holders, and the registered/running/idle
   transitions. Stdio grants now have an idle window of their own (30 min default,
   `MCP_LOCATOR_GRANT_IDLE_SECS`): the relay records when bytes last moved, so a grant a client
   forgot to release is reclaimed rather than held for the life of the session. Idleness is
   *observed* there, which is why it applies to stdio only — an endpoint-mode client talks to its
   server directly and the broker would be inventing a number it cannot see.
   Still open: server-crash detection and the `runtime.json` snapshot + reconcile-on-start.
5. **Consent** — *done*: `consent.json` writer with atomic replace, `launchHash` binding with
   stale detection, enforcement at activation, and the Win32 TaskDialog helper
   (`crates/consent-ui`) showing the server, its launch command, the card's origin and tier, and
   the requesting process — the last read from the PID rather than self-reported. Activation
   raises it; the answer comes back as an exit code, so nothing a client sends reaches the
   dialog. Stale approvals re-prompt with the old and new commands side by side, which is why
   the record now stores `launchCommand` next to the hash. Prompts are serialized machine-wide
   and low-integrity clients cannot raise one at all.
   Still open: allow-for-this-client scope (`ConsentScope::Client` exists but nothing writes it).
6. **Bootstrap hardening** — *mostly done*: the MSI puts the broker under `%ProgramFiles%`, the
   client library refuses to launch one from anywhere else, and both checks spec/003 §3 calls
   for are now enforced — path containment and Authenticode. `MCP_LOCATOR_ALLOW_UNSIGNED_BROKER=1`
   is the development escape hatch. Still open: the singleton named mutex, `admin/supersede`,
   and drain/handover.
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
   rules enforced: system-tier card only, command must resolve inside the install root, and
   Authenticode verified before launch (`MCP_LOCATOR_ALLOW_UNSIGNED_BROKER=1` to override for a
   development build). All four verification outcomes were proven on a sandbox with a self-signed
   certificate: unsigned refused, signed accepted and named, a one-byte edit caught as "modified
   after it was signed", and the publisher surfaced in the consent dialog.
5. ✔ Onboarding docs in the README and INSTALL.md, and the gateway has now been driven end to end
   by a third-party AI client (see "Proven against a real client" below).

The gateway ships as a single bundled `.mjs` (esbuild) so the MSI can install it without a
`node_modules` tree. That required the client library's two `createRequire` calls to become
static imports — a bundler cannot follow a runtime `require`, and the failure would have been a
gateway that worked from a checkout and not from an install.

Exit criterion met end to end: a real MCP client sees only the three meta-tools, discovers the
demo server, activates it, watches `notes.echo` / `notes.add` appear mid-session, calls one and
reaches the child process, then deactivates and watches them disappear.

## Proven against a real client, and against scale

Two exercises after M3, on the same disposable Windows 11 VM. Both changed the plan.

**A third-party AI client drove it end to end.** Hermes — an independent MCP client, unmodified
and told nothing but `node` and one path — discovered two registered servers, activated one, called
it, and released it. The broker's audit log records the grant lifecycle independently of anything
the model said, which is what makes it a result rather than an anecdote: one server activated and
released 49s apart, its neighbour never touched.

**A thousand registered servers, and an agent churning a hundred of them.** This is where the
design's central claim was actually tested, and where most of the following work came from.

| Measured with 1000 servers registered | |
|---|---|
| Tool-list floor, nothing activated | **1,948 B** — the whole premise, in one number |
| `list_servers`, default compact page of 50 | 4,371 B (was ~326 KB for all rows, pretty-printed) |
| One 100-tool server, fully activated | 33,157 B |
| The same server, `tools: ["ping"]` | 2,499 B |
| 100 × activate → call → deactivate | 41 s, 0 failures, every grant paired |
| activate / tool call / deactivate | 355 ms / 2 ms / 21 ms |

What that produced, all now done:

- **Discovery is paged and searchable.** `list_servers` defaults to one compact line per server and
  takes `query`, `limit` and `cursor`. Returning every row was worse than the problem this project
  exists to solve — 83k tokens for the one call a model always makes first.
- **Activation is selective.** `activate` takes a `tools` filter, so a server offering a hundred
  tools need not spend a hundred schemas to be useful. An unknown name is an error rather than a
  silent omission, and a withheld tool is refused at call time so the filter is not advisory.
- **The gateway stopped claiming what its client would call things.** `activate` used to advertise
  `<alias>.<tool>`; Hermes renames tools under its own prefix, the model believed us, and it spent
  two round trips recovering from a name that did not exist. The gateway is not in the call path
  and is in no position to say. `tools/list_changed` already tells the client to re-read.
- **The activation message stopped spending what activation saved** — a 100-tool server produced a
  1.4 KB reply naming every tool. Every unit test passed while that was true; nothing in them had
  more than three tools.
- **Forgotten grants are reclaimed** (M2.4 above). The agent held two of its hundred servers for
  40 and 25 minutes, then reported it had released everything.
- **The gateway exits when its client does.** It handled SIGINT/SIGTERM, but a client that dies
  sends no signal — stdin simply closes. Every crashed client stranded a gateway holding a broker
  connection. The reaper alone would not have covered this: grants would be reclaimed while the
  process that took them lived on.
- **The gateway drops servers whose relay closes**, so a reclaimed grant does not leave a model
  holding tool names with nothing behind them.

Two lessons worth keeping rather than just the fixes:

**A model's self-report is not evidence.** The agent stated, with a described verification method,
that it had kept one server active at a time. The audit log showed otherwise. Keep the invariant
that makes that checkable — one `release` for every `activate`, even when the broker reclaims a
grant itself — and prefer the broker's record to the model's summary in every test.

**Scale is a different test, not a bigger one.** Three of the items above passed the entire unit
suite. They were only reachable with a thousand cards and a server with a hundred tools.

### Still open from this work

- **Tool manifests in the card** (biggest remaining item; needs a schema change, all three parsers,
  and conformance vectors). `query` searches a one-line server description today, so a server whose
  blurb says "productivity suite" is unfindable even though it has a `send_email` tool. A declared
  manifest would make search useful *and* let a model use `activate(tools: […])` without first
  guessing names or activating once to see them. Drift is the cost: the manifest is a hint, and the
  running server stays authoritative.
- **`list_servers` should show what the caller already holds.** The agent had no cheap way to notice
  it had leaked two grants. `status: active` is in the listing but nothing draws a client to it.
- **Broker-side narrowing.** `locator/list` ships every card over the pipe — 349 KB and ~1 s for a
  thousand — and the gateway then renders 6 KB of it. Never a context cost, but the gateway should
  not be the only place that narrows.
- **`consent grant` has no bulk form.** Approving a thousand servers took 198 s of sequential
  process spawns.
- **`DEFAULT_LIMIT = 50` is a guess.** Nothing measured says fifty is the right page.
- **An orphaned card should say why.** A card whose command will not resolve reports
  `orphaned: true` and no reason. Installing Node and a server in one session leaves a stale `PATH`
  in the calling process, and the card reads as broken when the environment is at fault.
- **Models still guess tool names** even when told to read their own list. Worth watching; not
  obviously ours to fix.

## M4 — Federation + upstream (incremental, in value order)

1. **MSIX `appExtension` provider** (highest local value: the `package` trust tier).
2. **Broker port** to macOS/Linux (unix sockets, codesign verification, launchd/systemd units).
3. **Remote catalogs**: SEP-2127 `.well-known` fetcher; **mDNS** `_mcp._tcp` listener (separate
   consent class per spec/003).
4. **Windows ODR provider**: wrap `odr.exe` / the host API; revisit when ODR reaches GA.
5. **Upstream**: propose "local server cards" as an extensions-track SEP — the spec/001 diff
   against SEP-2127 plus the running implementation.

## Where this stands

M0, M1 and M3 are done. M2 met its exit demo and is down to a tail. Everything below is what
remains; nothing else in this document is outstanding.

| | Remaining |
|---|---|
| **M2.1 / 2 / 7** | `subscribe` + the notify-crate file watcher, and the client-library subscriptions that ride on them. One feature across three steps: today a catalogue change is invisible until something re-lists. |
| **M2.4** | Server-crash detection, `runtime.json` snapshot and reconcile-on-start. A broker restart currently forgets what was running. |
| **M2.5** | Per-client consent scope — `ConsentScope::Client` exists in the store and nothing writes it. |
| **M2.6** | Singleton named mutex, `admin/supersede`, drain/handover. Two brokers can still race. |
| **M2.8** | CLI verbs against a live broker: `activate`, `deactivate`, `status`. The broker has `serve`, `list`, `dirs`, `verify`, `consent`, `secure-dirs`. |
| **M2.9** | An OV/EV certificate. The only item needing someone other than us; `-Sign` is already wired through `installer/build.ps1`. |
| **M3.2** | Mirror resources and prompts, not just tools. |
| **Scale work** | Card tool manifests, and the smaller items listed above. |
| **M4** | All five items, untouched. |

Sequencing: the certificate is the only external dependency and gates a public release, so start
it now regardless of what else is in flight. Tool manifests are the largest remaining design
change and touch the schema, so they want doing before M4 hardens anything around cards.
`subscribe` + watching is the last piece of the original M2 that a user would notice.

## Cross-cutting

- **Versioning**: schema version in `$schema` URL; `brokerProtocol` integer; additive-only within
  v1. Conformance fixtures are tagged by schema version.
- **Testing pyramid**: conformance fixtures (shared) → Rust integration tests spawning real
  processes → one end-to-end smoke on Windows CI (broker + TS client + fake server).
- **Cross-target lint before pushing.** Half the broker is behind `cfg(windows)`, so a
  Windows-only `cargo clippy` cannot see the other half — code reachable only from the Windows
  module reads as dead on unix. Catch it locally with
  `cargo clippy --all-targets --target x86_64-unknown-linux-gnu -- -D warnings`
  and the same for `x86_64-apple-darwin` (`rustup target add` once; no linker needed, since
  clippy does not link). Note the limits: this catches compile and lint failures, not runtime
  ones. `/proc` exists on Linux and not on macOS, and only CI running the tests found that.
- **Security gates**: spec/003 checklist review before the first binary release; fuzz the card
  parser and the pipe framing (cheap, high value — both parse attacker-controlled input).

## Verified on a clean machine

M2 hardening was signed off against a disposable Windows 11 VM with nothing on it — no Node, no
VC++ runtime, no developer tools. Four bugs surfaced there that no amount of testing on a
developer's machine could have found, because each one is masked by something a dev box already
has:

| Symptom on a clean machine | Cause | Fix |
|---|---|---|
| MSI fails with 1603, naming nothing | broker linked the MSVC runtime dynamically; stock Windows has no `VCRUNTIME140.dll` | static CRT (`broker/.cargo/config.toml`), asserted on the artefact in `build.ps1` |
| static CRT silently not applied | cargo reads `.cargo/config.toml` relative to the **working directory**, not `--manifest-path` | `build.ps1` builds from inside `broker/` |
| consent dialog opened *behind* the client | `SetForegroundWindow` cannot take the foreground from a background service | dialog sets itself topmost, as a modal security prompt should |
| reinstalling a rebuild changed nothing, exit 0 | WiX assigns a new ProductCode per build; `MajorUpgrade` refuses an unchanged version | `AllowSameVersionUpgrades="yes"` |

Keep the loop: build the MSI, install it on a fresh sandbox, and run the end-to-end check there
before calling a release done. The check must assert rather than print — the gateway reports
refusals as tool *content*, so a script that only catches thrown errors passes a run in which
nothing worked.

Two more bugs came from the scale run rather than a clean install, and they generalise the rule:
the activation message that spelled out a hundred tool names, and the gateway that outlived its
client. Both passed every unit test. Add a fixture with a hundred tools and a run that kills a
client mid-session; a suite whose largest server has three tools cannot see either.

## Risks

| Risk | Mitigation |
|---|---|
| No Authenticode cert early on | `MCP_LOCATOR_DEV=1` mode; buy OV/EV cert before first public MSI; Sigstore as interim for non-Windows |
| Stdio relay latency/backpressure bugs | keep relay dumb (single duplex copy loop); e2e test with large payloads early in M2.3 |
| launchHash canonicalization drift between Rust and TS | shared test vectors in `conformance/`; canonicalization spec'd byte-exact (JCS / RFC 8785) |
| Consent UI spoofing concerns | helper is signed, launched only by broker, renders on interactive desktop; document in spec/003 |
| ODR ships GA sooner than expected | architecture already treats it as a provider; no bet depends on it staying preview |

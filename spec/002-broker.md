# 002 — The Broker

Status: Draft
Layer: effects (everything with side effects goes through the broker)

## 1. Role

The broker is a per-user, on-demand daemon — the RPCSS/SCM analogue to the card registry's
"registry keys." It owns exactly four things:

1. **Authoritative runtime state** — which servers are running, activating, or idle.
2. **Activation** — consent check, launch, connection handoff, refcounted grants.
3. **Lifetime** — idle timeout, client-death cleanup, graceful shutdown.
4. **The consent store** — the single record of user decisions, shared by all AI clients.

It owns **no registration data**. The catalog is derived by file-watching the card directories
(spec/001 §2) plus, later, federation providers (mDNS, remote catalogs, MSIX `AppExtensionCatalog`,
Windows ODR). Killing the broker and deleting its state directory loses nothing but runtime state.

## 2. Bootstrap

The broker is registered as a card like any other server: `io.mcplocator.broker.card.json`,
**valid only in the `system` tier** (a broker card found in `user` or `low` is ignored and
reported — see spec/003 §3).

Client-library connect sequence:

```
1. connect to \\.\pipe\mcp-locator\broker\v1        (unix: $XDG_RUNTIME_DIR/mcp-locator/broker.sock)
2. on ENOENT/refused:
   a. read the broker card from the system tier only
   b. verify launch.command resolves under the broker's install root and carries a
      valid Authenticode/codesign signature matching the configured publisher
   c. launch it; the broker takes the named mutex "mcp-locator-broker" (singleton)
   d. retry the pipe with backoff (fail after ~5s)
3. handshake
```

### Handshake and version election

`locator/handshake { libVersion, brokerProtocol: 1 }` → `{ brokerVersion, brokerProtocol }`.

Client libraries bundle a broker binary, so version skew is normal. Rule: **highest version
wins.** A library whose bundled broker is newer than the running one may call
`locator/admin/supersede { candidatePath, version }`; the running broker verifies the candidate's
signature, spawns it, drains (stops accepting activations, hands off live grant bookkeeping via
its state file, releases the mutex), and exits. Protocol changes are additive within `brokerProtocol: 1`.

## 3. Transport and protocol

JSON-RPC 2.0 over the named pipe / unix socket, newline-delimited. The pipe ACL grants connect
to the owning user's SID, including low-IL and AppContainer clients (policy decides what a
low-trust *client* may activate — see spec/003 §5 — but discovery is open to the user's session).

### Methods

| Method | Params → Result | Notes |
|---|---|---|
| `locator/handshake` | versions → versions | first call on every connection |
| `locator/list` | `{ includeState }` → `[CatalogEntry]` | entries carry `trust`, `state`, `consent` |
| `locator/subscribe` | `{}` → stream of notifications | `catalogChanged`, `serverStateChanged` |
| `locator/activate` | `{ name }` → `{ grantId, connection }` | the core call; see §4 |
| `locator/release` | `{ grantId }` → `{}` | drop one grant |
| `locator/deactivate` | `{ name, force? }` → `{}` | manual: drops **all** grants, stops the server |
| `locator/status` | `{ name }` → `{ state, pid?, grants, since }` | authoritative liveness |
| `locator/consent/query` | `{ name }` → `{ state, grantedAt?, cardHash? }` | read; writes only via activation flow |
| `locator/admin/supersede` | see §2 | version election |

`CatalogEntry.state` ∈ `registered | launching | running | idle | stopping | orphaned`.
`consent.state` ∈ `granted | denied | not-asked | stale` (`stale` = card's launch stanza changed
since consent was given; see spec/003 §4).

### `locator/activate` semantics

1. Look up the card; if consent is `not-asked` or `stale`, show the consent UI (a small helper
   process on the user's desktop, since the broker itself is headless). `denied` → error.
2. Ensure the server is running:
   - `launch.type: "stdio"` — the broker spawns the process and owns its stdio. The MCP byte
     stream is exposed to the client over a fresh per-activation pipe
     (`\\.\pipe\mcp-locator\conn\<grantId>`); the broker relays pipe⇄stdio. One server process
     serves all concurrent grants only if the server speaks a multiplex-capable transport;
     otherwise the broker spawns one process per grant (card flag `local.launch.shared`, default
     false for stdio).
   - `launch.type: "executable"` / endpoint-only — ensure the process is up (spawn if `launch`
     present and not running), then return the card's endpoint directly.
3. Register the grant: `(clientPid, name) → grantId`. The broker duplicates the client's process
   handle and waits on it — client dies ⇒ all its grants are released automatically. This is the
   COM dead-client garbage-collection model, verbatim.
4. Return `{ grantId, connection: { type, address } }`. The client speaks plain MCP to that
   address; the broker is not in the data path for endpoint-mode servers and is a dumb byte relay
   for stdio-mode ones.

## 4. Lifetime state machine

```
                 activate (first grant)
  registered ───────────────────────────► launching ──► running(grants ≥ 1)
      ▲                                                     │
      │                                     last grant released / client PID died
      │                                                     ▼
      │            idleTimeoutSeconds elapsed          idle(grants = 0)
      └── stopping ◄────────────────────────────────────────┘
              (graceful: close stdio / signal; kill after grace period)

  any state ──► registered   on server PID death (crash): grants notified via
                             serverStateChanged, then invalidated
  any state ──► stopping     on locator/deactivate {force: true}
```

- A new `activate` during `idle` cancels the shutdown timer (refcount goes back above zero).
- `deactivate` without `force` refuses while other clients hold grants and returns who they are;
  with `force` it notifies all grant holders, then stops the server.
- On broker startup it reconciles: reads its state file, re-checks recorded PIDs, adopts
  still-running servers it previously launched, and marks the rest `registered`.

## 5. State directory

`%LOCALAPPDATA%\mcp-locator\state\` (user-tier): `runtime.json` (crash-safe snapshot of grants
and PIDs, for supersede/restart reconciliation), `consent.json` (spec/003 §4), `audit.log`
(append-only: activations, consent decisions, shutdowns — one JSON line each). All world-readable
within the user session; written only by the broker.

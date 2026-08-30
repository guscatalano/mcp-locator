# mcp-locator

**Local discovery and activation for MCP servers.** Any app on the machine can make its MCP
server discoverable by every AI client — without the user hand-editing a config file per client.

## The problem

Today, every AI client (Claude Desktop, Claude Code, Cursor, VS Code, …) has its own MCP config
file, and every app that ships an MCP server has to tell users to edit N of them. There is no
standard for same-machine discovery: SEP-2127 (server cards) covers remote HTTP discovery via
`.well-known`, the Windows On-device Agent Registry (ODR) is Insider-preview and Windows-only,
and mDNS proposals cover the LAN. The same-machine gap is what mcp-locator fills.

## Architecture

Two layers, modeled on COM's split between passive registration (the registry) and active
machinery (RPCSS/DcomLaunch):

```
┌─────────────────────────────────────────────────────────────┐
│  AI clients (Claude, Cursor, …)                             │
│    └── locator client library  ──────────────┐              │
│          reads cards directly (brokerless)   │ effects      │
└──────────────────────────┬───────────────────┼──────────────┘
                           │ read-only         │ JSON-RPC pipe
┌──────────────────────────▼───────┐  ┌────────▼──────────────┐
│  Card registry (files on disk)   │  │  Broker (daemon)      │
│  machine / user / low-IL dirs    │◄─┤  watches card dirs    │
│  written by apps at install      │  │  launch · consent ·   │
│  SOURCE OF TRUTH: registration   │  │  refcounts · state    │
└──────────────────────────────────┘  │  SOURCE OF TRUTH:     │
                                      │  runtime state        │
                                      └───────────────────────┘
```

- **Card registry (spec/001):** apps drop one JSON *server card* (SEP-2127-aligned schema plus a
  `local` block) into a well-known directory at install time. Registration works while the app is
  not running, survives broker reinstalls, and is readable by anyone with just the library.
- **Broker (spec/002):** an on-demand, per-user daemon that owns everything with side effects —
  launching servers, the shared consent store, activation refcounts, idle/PID-death cleanup, and
  authoritative running-state. Its catalog is entirely *derived* from the card files; it can be
  killed and rebuilt with zero data loss.

### The capability split: reads are free, effects go through the broker

| Capability                          | Brokerless (library) | Broker |
|-------------------------------------|----------------------|--------|
| Enumerate catalog, read metadata    | ✔                    | ✔      |
| Liveness                            | best-effort (`probablyRunning`) | authoritative |
| Read consent state                  | ✔                    | ✔      |
| Launch / activate a server          | —                    | ✔      |
| Grant or deny consent               | —                    | ✔      |
| Refcounted lifetime, idle shutdown  | —                    | ✔      |
| Catalog/state change notifications  | file-watch only      | ✔ (subscriptions) |
| LAN / remote / ODR federation       | —                    | ✔ (future providers) |

Brokerless activation is deliberately not allowed: two clients spawning the same server
independently means double-spawn, orphaned processes, and no shared consent.

### Bootstrap

The broker is itself registered as a card (`io.mcplocator.broker`), so any client library knows
how to start it: read cards → need an effect → connect to the broker pipe → pipe absent → launch
the broker (singleton via named mutex) → connect. The broker card is honored **only** from the
machine-tier directory and its binary is signature-checked before launch — see spec/003 for why
this rule is load-bearing.

## Activation model

An AI client (or the AI itself, via the gateway) *activates* a server: the broker checks consent,
launches the process if needed, and hands back a connection. The activation is a refcounted grant
tied to the client's PID. Deactivation is manual (`release`/`deactivate`) or safe-automatic:
idle timeout after the last grant is released, client PID death (the broker waits on the process
handle, exactly as COM garbage-collects dead clients), or server PID death.

## Install

```powershell
.\installer\build.ps1
msiexec /i installer\dist\mcp-locator-0.1.0-x64.msi
```

Windows x64; the gateway needs Node 20.10+ on `PATH`. Full details, including how an app
registers a server and how consent works, are in [INSTALL.md](INSTALL.md).

## Try it

Configure **one** MCP server in your AI client, once:

```jsonc
// Claude Desktop: claude_desktop_config.json — Cursor / VS Code / Claude Code use the same shape
{
  "mcpServers": {
    "mcp-locator": {
      "command": "node",
      "args": ["C:\\Program Files\\mcp-locator\\gateway\\mcp-locator-gateway.mjs"]
    }
  }
}
```

From then on the client sees three tools — `list_servers`, `activate`, `deactivate` — and every
server registered on the machine shows up through them, including ones installed *after* the
client was configured. Activating one makes its tools appear in the same session:

```
tools before activation: list_servers, activate, deactivate
activate com.example.notes
  → Activated. 2 tool(s) now available as notes.*: echo, add
tools after activation:  list_servers, activate, deactivate, notes.echo, notes.add
```

Nothing needs to be running first: the gateway finds the broker's card in the system-tier
directory and starts it. Activating a server the user has not decided on raises a dialog, which
the AI cannot answer — approval is always a human click (or an out-of-band
`mcp-locator-broker consent grant`).

To see the whole stack from a source checkout:

```bash
npm install && npm run build
cargo build --manifest-path broker/Cargo.toml

broker/target/debug/mcp-locator-broker serve &
node packages/gateway/scripts/e2e-full-stack.mjs com.example.notes
```

`packages/gateway/scripts/demo-server.mjs` stands in for an installed app that ships an MCP
server; register it by dropping a card in the user-tier directory (`mcp-locator dirs` prints the
paths).

## Roadmap

1. **M1 — Cards + read-only library.** Card schema, directory layout, brokerless enumeration and
   liveness hints. Useful immediately; trivially cross-platform.
2. **M2 — Broker.** ✔ Activation, consent dialog, refcounted lifetime, authoritative state,
   bootstrap, and an MSI that installs all of it with the directory permissions the bootstrap
   depends on. Signature verification of the broker binary is the remaining gap.
3. **M3 — Gateway shim.** ✔ A thin MCP server (`list_servers` / `activate` / `deactivate`
   meta-tools, dynamic tool re-export with `tools/list_changed`) so unmodified AI clients get the
   full experience by configuring exactly one server, ever.
4. **M4 — Federation providers.** mDNS (`_mcp._tcp`), remote `.well-known` catalogs (SEP-2127),
   MSIX `appExtension` registration, and the Windows ODR when it reaches general availability.

## Spec

- [spec/001-cards-and-registry.md](spec/001-cards-and-registry.md) — card format, directories, trust tiers, liveness
- [spec/002-broker.md](spec/002-broker.md) — bootstrap, pipe protocol, activation lifecycle
- [spec/003-security.md](spec/003-security.md) — threat model, consent binding, integrity levels
- [INSTALL.md](INSTALL.md) — building, installing, registering a server, managing consent
- [examples/](examples/) — sample cards

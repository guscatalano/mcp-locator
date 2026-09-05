# 001 — Local Server Cards and the Card Registry

Status: Draft
Layer: brokerless (readable with the client library alone)

## 1. Overview

A *local server card* is a single JSON file that registers one MCP server with the machine. Apps
write their card at install time (or first run) into a well-known directory; anything on the
machine can enumerate the directories to discover every registered server — whether or not the
owning app is running. The card files are the **single source of truth for registration**. The
broker (spec/002) never stores registration data of its own; its catalog is derived by watching
these directories.

The card format is deliberately a superset of the SEP-2127 MCP Server Card
(`io.modelcontextprotocol/server-card`): identity fields are identical, remote endpoints reuse
the SEP-2127 `remotes` shape, and local-machine specifics live in one added `local` block. A
future "local server cards" SEP should be a small diff against SEP-2127, not a new format.

## 2. Registry directories

One card per file, named `<name>.card.json` (e.g. `com.contoso.slack.card.json`). A file whose
basename does not match the card's `name` field is ignored (prevents casual squatting).

| Tier | Windows | macOS | Linux |
|------|---------|-------|-------|
| `system` (admin-writable) | `%ProgramData%\mcp-locator\servers\` | `/Library/Application Support/mcp-locator/servers/` | `/usr/share/mcp-locator/servers/` (+ `$XDG_DATA_DIRS`) |
| `user` | `%LOCALAPPDATA%\mcp-locator\servers\` | `~/Library/Application Support/mcp-locator/servers/` | `~/.local/share/mcp-locator/servers/` |
| `low` (low-integrity writable) | `%USERPROFILE%\AppData\LocalLow\mcp-locator\servers\` | — | — |

Rules:

- The tier a card was found in is part of its identity (`trust.tier` in the merged catalog) and
  is surfaced to users. `system` requires admin rights to write; `low` exists so sandboxed/low-IL
  processes can register at all, and everything found there is presented as low-trust.
- If the same `name` appears in multiple tiers, the higher tier wins; the shadowed card is
  reported as a conflict, not silently merged.
- **MSIX caveat:** packaged apps' writes to `%LOCALAPPDATA%` are virtualized into the package
  container and are invisible to other processes. Packaged apps must either write the `system`
  tier from their installer, or (preferred, M4) declare a `windows.appExtension` that the broker's
  MSIX provider enumerates via `AppExtensionCatalog`. Manifest-declared registration is also
  auto-removed on uninstall and tamper-evident (signed manifest), so it will map to a trust tier
  *above* `system`.

## 3. Card schema

```jsonc
{
  // SEP-2127-aligned identity (same field names and semantics)
  "$schema": "https://mcp-locator.dev/schemas/v1/local-server-card.schema.json",
  "name": "com.contoso.slack",          // reverse-DNS, must match filename
  "version": "1.2.0",
  "description": "Send and read Slack messages.",
  "title": "Slack",
  "icons": [ { "src": "file:///C:/Program Files/Contoso/slack-mcp.png", "mimeType": "image/png" } ],
  "websiteUrl": "https://contoso.com",

  // SEP-2127 remotes block, verbatim — a local app may ALSO have a cloud endpoint
  "remotes": [],

  // the local extension
  "local": {
    // How to start the server when it is not running. Optional: endpoint-only cards
    // describe servers that only exist while their app is running.
    "launch": {
      "type": "stdio",                   // "stdio" | "executable"
      "command": "C:\\Program Files\\Contoso\\slack-mcp.exe",
      "args": ["--serve"],
      "cwd": null,
      "env": {}
      // type "stdio": the launched process speaks MCP on stdin/stdout.
      // type "executable": the process starts and then serves on local.endpoint;
      //   used when launch brings up a headless app that owns the endpoint.
    },

    // Where to connect when the server IS running. Required for type "executable"
    // and for endpoint-only cards; absent for pure stdio.
    "endpoint": {
      "type": "pipe",                    // "pipe" (Windows) | "unix-socket" | "streamable-http"
      "address": "\\\\.\\pipe\\contoso-slack-mcp"
      // streamable-http addresses must be loopback. Dynamic ports: the app rewrites
      // its card (or the portFile below) at startup — cards are cheap to rewrite and
      // watchers pick the change up.
    },

    // Brokerless liveness hints. Best-effort by definition (see spec/002 §5 for the
    // authoritative version). All optional.
    "liveness": {
      "pidFile": "%LOCALAPPDATA%\\Contoso\\slack-mcp.pid",
      "probe": true                      // readers may test-connect the endpoint
    },

    // Lifetime preferences, enforced by the broker.
    "lifetime": {
      "idleTimeoutSeconds": 300,         // shutdown delay after last grant released
      "shutdown": "graceful"             // "graceful" (close stdio / signal, then kill
                                         // after grace period) | "kill"
    },

    // Consent metadata shown in the broker's consent prompt.
    "consent": {
      "summary": "Can read and send messages in your Slack workspace."
    }
  },

  "_meta": {}                            // namespaced vendor extensions, per SEP-2127
}
```

Required fields: `name`, `version`, `description`, and at least one of `local.launch`,
`local.endpoint`, or a non-empty `remotes`. Everything else is optional.

Environment-variable expansion is applied to `command`, `args`, `cwd`, `launch.env` values,
`endpoint.address`, and `liveness.pidFile` at read time, in the *reader's* environment. Both
`%VAR%` and `${VAR}` are accepted on every platform, so one card file stays portable. Unknown
variables are left verbatim rather than expanded to empty — silently emptying a path would turn
a typo into a launch of the wrong file.

### Resolving `launch.command`

A command containing a path separator is a path and is checked as one. Anything else is a bare
name, resolved through `PATH` (and `PATHEXT` on Windows) exactly as the OS would resolve it at
launch. A card may therefore say `"command": "node"`, which is more portable than hard-coding an
interpreter path that differs on every machine.

The distinction is load-bearing in both directions. Treating a bare name as a missing file hides
a perfectly runnable server as an orphan, with no diagnostic to explain the disappearance.
Searching `PATH` for something written *as* a path is worse: a missing `./tools/notes.exe` could
be quietly satisfied by an unrelated `notes.exe` earlier on `PATH` — the wrong program, launched
under a name the user has already approved.

### File encoding

Cards are UTF-8 JSON. A leading byte-order mark **must** be accepted and ignored, even though
JSON forbids it: .NET and Windows PowerShell write one by default, so the tools a Windows app
developer reaches for to author a card produce one without saying so, and the resulting failure
is invisible in an editor. Every implementation strips it — `conformance/fixtures/basic/user/
com.example.bom.card.json` is the shared proof.

## 4. Brokerless reads

The client library exposes, without any broker:

- `enumerate()` — merged catalog across tiers with per-card `trust.tier`, conflict flags, and
  parse errors surfaced (a malformed card is reported, not silently dropped).
- `probablyRunning(card)` — best-effort: pidFile exists and the PID is alive, and/or the endpoint
  accepts a connection. The name is the contract: brokerless liveness may be stale or racy, and
  API consumers must not treat it as authoritative.
- `readConsentState(name)` — read-only view of the broker's consent store (spec/003 §4).
- File-watching over the registry directories for catalog-change callbacks.

Anything with side effects — launching, consent decisions, activation — is broker-only
(spec/002). Libraries MUST NOT spawn `local.launch` themselves.

## 5. Registration lifecycle for app developers

1. Installer writes `<name>.card.json` to the appropriate tier directory.
2. If the endpoint uses a dynamic port, the app rewrites the card's `endpoint` (atomically:
   write temp file, rename) each time it starts.
3. Uninstaller deletes the card. Because uninstallers are unreliable, readers treat a card whose
   `launch.command` no longer exists on disk as *orphaned* and hide it from default listings.

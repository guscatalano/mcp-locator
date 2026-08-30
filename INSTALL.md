# Installing mcp-locator

Windows x64. Node 20.10 or later must be on `PATH` — the gateway runs on it. Nothing else does,
so a machine that will only *register* servers (an app shipping a card) needs no Node at all.

## Build the MSI

```powershell
.\installer\build.ps1              # -> installer\dist\mcp-locator-0.1.0-x64.msi
.\installer\build.ps1 -Sign cert.pfx -SignPassword (Read-Host -AsSecureString)
```

Without `-Sign` the MSI and both executables are unsigned. That is fine on a machine you
control and wrong for anything you hand to someone else — see [Signing](#signing).

## Install

```powershell
msiexec /i mcp-locator-0.1.0-x64.msi          # UAC prompt; add /qn for silent
msiexec /x mcp-locator-0.1.0-x64.msi          # uninstall
```

What lands where, and why:

| Path | Contents |
|---|---|
| `%ProgramFiles%\mcp-locator\` | broker, consent helper, gateway bundle |
| `%ProgramData%\mcp-locator\servers\` | system-tier registry — **administrators only** |
| `%LOCALAPPDATA%\mcp-locator\servers\` | user-tier registry, created on first broker run |
| `%APPDATA%\..\LocalLow\mcp-locator\servers\` | low-tier registry, labelled Low so sandboxed apps can register |
| `%LOCALAPPDATA%\mcp-locator\state\` | consent store and audit log, labelled Medium so sandboxes cannot |

The permissions are the point, not a detail. Client libraries will launch a broker **only** from
a system-tier card whose binary sits under the install root, so "only an administrator could have
put it there" is what the whole bootstrap rests on. Verify it after installing:

```powershell
icacls C:\ProgramData\mcp-locator\servers
```

Administrators and SYSTEM full; Authenticated Users and app packages read-only; no inherited
ACEs. Uninstalling leaves your consent decisions and audit log in `%LOCALAPPDATA%` — they are
yours, and a reinstall should not silently re-approve anything.

## Point your AI client at it

One entry, once, in any MCP client. Every server registered on the machine — including ones
installed later — arrives through it.

```jsonc
// Claude Desktop: %APPDATA%\Claude\claude_desktop_config.json
// Cursor, VS Code, and Claude Code use the same shape.
{
  "mcpServers": {
    "mcp-locator": {
      "command": "node",
      "args": ["C:\\Program Files\\mcp-locator\\gateway\\mcp-locator-gateway.mjs"]
    }
  }
}
```

Restart the client. It will see three tools — `list_servers`, `activate`, `deactivate` — and
starting the broker is automatic from there; nothing needs to be running beforehand.

## Registering a server

An app makes itself discoverable by dropping one file. No API, no running process:

```powershell
$dir = "$env:LOCALAPPDATA\mcp-locator\servers"
New-Item -ItemType Directory -Force $dir | Out-Null
# The filename must match the `name` field exactly, or the card is rejected.
[IO.File]::WriteAllText("$dir\com.example.notes.card.json", @'
{
  "name": "com.example.notes",
  "version": "1.0.0",
  "description": "Search and create notes.",
  "title": "Example Notes",
  "local": {
    "launch": {
      "type": "stdio",
      "command": "C:/Program Files/Example Notes/notes-mcp.exe",
      "args": ["--serve"]
    },
    "lifetime": { "idleTimeoutSeconds": 300, "shutdown": "graceful" },
    "consent": { "summary": "Can read, search, and create notes in your library." }
  }
}
'@)
```

Then check what the machine sees:

```powershell
& "C:\Program Files\mcp-locator\mcp-locator-broker.exe" list
```

Two things reject a card quietly enough to be worth naming:

* **`launch.command` must be a path that exists.** A bare `node` is treated as a missing program
  and the card is hidden as orphaned. Write the full path.
* **The filename must be `<name>.card.json`.** Anything else is reported as a mismatch rather
  than trusted, so a card cannot claim a name it did not file under.

## Consent

The first time any client activates a server, the broker raises a dialog naming the server, the
program it will start, where the card came from, and which process asked. Nothing an AI client
sends reaches that dialog, and an AI cannot answer it.

Decisions are bound to the launch command, not the server name. If a card's command changes
afterwards the approval goes stale and you are asked again, with the old and new commands shown
side by side — that is what stops an approved card being quietly repointed at something else.

Out-of-band management, for scripted setup or for undoing a click:

```powershell
$broker = "C:\Program Files\mcp-locator\mcp-locator-broker.exe"
& $broker consent list
& $broker consent grant com.example.notes    # pre-approve, no dialog
& $broker consent deny  com.example.notes
& $broker consent forget com.example.notes   # back to unasked
```

Setting `MCP_LOCATOR_NO_PROMPT=1` before starting the broker disables prompting entirely;
activation then fails with `CONSENT_REQUIRED` unless consent was granted out of band. That is
the mode for headless machines and CI.

Everything the broker does is appended to `%LOCALAPPDATA%\mcp-locator\state\audit.log`, one JSON
object per line — grants, denials, activations, releases, and refusals.

## Signing

The build is unsigned by default and SmartScreen will warn on any machine that is not the one
that built it. For real distribution you need an OV or EV code-signing certificate, then:

```powershell
.\installer\build.ps1 -Sign path\to\cert.pfx -SignPassword (Read-Host -AsSecureString)
```

That signs both executables and the MSI, with a timestamp so they stay valid past the
certificate's expiry.

Worth being clear about what is still missing: the client library checks that a broker binary
lives under the install root before launching it, but it does not yet verify the signature.
Path containment is a real check — writing to `%ProgramFiles%` needs administrator rights — but
it is one check where the design calls for two, and the second is not there yet.

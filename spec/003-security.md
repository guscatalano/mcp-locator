# 003 — Security Model

Status: Draft

## 1. Threat model

A card containing a `launch` stanza is an arbitrary-code-execution instruction, and the registry
directories are (by design) writable by ordinary processes. The system therefore never treats a
card as trustworthy — a card is an *advertisement*. The security boundaries are:

1. **Nothing launches without user consent** (per server, bound to the card's launch stanza).
2. **Only the broker launches server cards.** Clients launch exactly one binary directly — the
   broker — under the hardened rule in §3.
3. **Trust tiers are provenance, not permission.** They inform the consent UI and default
   policy; they never skip consent.

Non-goals (stated honestly): mcp-locator does not defend the user against *already-running
malware at the user's own integrity level*. Same-user medium-IL code can write the user-tier card
directory and the consent store, as it can tamper with browser profiles or `mcp.json` today. The
defenses below target the realistic threats: low-integrity/sandboxed processes escalating,
drive-by card drops that turn into silent code execution, consent given once being silently
repurposed, and cross-user attacks.

## 2. Trust tiers

| Tier | Write requirement | Presented as |
|---|---|---|
| `package` (M4: MSIX `appExtension`) | signed package install | highest — signed, auto-unregistered, tamper-evident |
| `system` | administrator | high |
| `user` | user's own IL | normal |
| `low` | low IL / sandbox | explicitly low-trust badge; consent UI warns |
| `lan` / `remote` (M4) | network presence | untrusted-by-default; separate consent class |

Additionally the broker records, per card, whether `launch.command` carries a valid code
signature and by whom; unsigned binaries get a consent-UI warning regardless of tier.

## 3. Broker bootstrap hardening

The "cards can start the broker" bootstrap must not become "any process that can drop a file
gets its binary launched by every AI client." Rules, all mandatory:

- The broker card is honored **only from the `system` tier** (admin-writable). Broker cards in
  `user`/`low` are ignored and surfaced as a warning.
- Before launching, the client library verifies that the resolved `launch.command` lies under
  the broker's install root (`%ProgramFiles%\mcp-locator\` by default) **and** that the binary's
  Authenticode/codesign signature matches the configured publisher identity. Both checks — path
  containment alone is defeated by a writable subdirectory; signature alone is defeated by any
  signed-but-hostile binary.
- `locator/admin/supersede` (spec/002 §2) applies the same signature check to the candidate
  binary before the running broker will hand over.
- Ordinary server cards get the opposite rule: client libraries MUST NOT launch them, ever.

*(Implemented.)* `WinVerifyTrust` in the broker (`mcp-locator-broker verify <path>` exposes it,
because a signature check nobody can run by hand is a check nobody can audit) and
`Get-AuthenticodeSignature` in the client library.

The client library deliberately does **not** call the broker's own `verify`. Asking a binary
whether it can be trusted is a question a tampered one answers "yes"; the verifier has to be
something the attacker would have had to subvert separately. PowerShell is invoked by absolute
path out of `System32` — resolving it through `PATH` would move the same problem down one level,
since `PATH` is user-writable — and `PSModulePath` is pinned to the in-box modules so nothing can
shadow `Get-AuthenticodeSignature` with its own.

Four outcomes, kept distinct because they mean different things:

| Result | Meaning | Bootstrap |
|---|---|---|
| `signed` | valid chain to a trusted root | launch |
| `unsigned` | no signature — an ordinary local build | refuse |
| `invalid` | signed but the file changed afterwards, or the root is untrusted | refuse |
| `unknown` | the check could not run | refuse |

`unknown` refusing is the load-bearing one: "could not check" is precisely the state an attacker
would like the check to end in, so it must never read as success.

`MCP_LOCATOR_ALLOW_UNSIGNED_BROKER=1` allows an unsigned broker for development, and prints why
on stderr each time. It is off by default because a check you have to opt into protects nobody.

All four paths were exercised on a clean VM with a self-signed certificate: unsigned refused,
signed accepted and named, a one-byte edit to the signed binary reported as modified-after-
signing and refused, and the signer surfaced in the consent dialog's Publisher row.

## 4. Consent

Stored in `consent.json` in the broker state directory; written only by the broker, readable by
the session. One record per server `name`:

```jsonc
{
  "com.contoso.slack": {
    "state": "granted",              // granted | denied
    "grantedAt": "2026-08-15T17:02:11Z",
    "launchHash": "sha256:…",        // hash of the card's canonicalized `local.launch` + `endpoint`
    "launchCommand": "…",            // the approved command line, for the stale diff
    "scope": "user"                  // "user" = all clients; "client" records add clientId
  }
}
```

- **The store is shared, and re-read.** `mcp-locator-broker consent grant/deny/forget` writes the
  same file a running broker holds, and that is the documented way to script an approval. A
  broker that loaded it only at startup got this wrong in both directions: it re-prompted for
  servers already approved on disk, and its next write persisted the stale map, silently erasing
  those decisions. The store now re-reads whenever the file has changed, and every write is a
  read-modify-write rather than a wholesale replacement.
- **Consent binds to `launchHash`.** If the card's launch stanza or endpoint changes, existing
  consent becomes `stale` and the user is re-prompted with a diff ("this server's launch command
  changed"). This is the rule that stops a benign registered card from being silently swapped
  for `cmd.exe /c …` after consent was given. Version bumps that don't touch launch/endpoint do
  not invalidate consent.
  `launchHash` proves *that* something changed; the stored `launchCommand` is what lets the
  prompt say *what*, which is the difference between a question a user can answer and two
  opaque digests.
- Default scope is per-user ("allow for all AI clients"); the consent UI offers per-client
  restriction, recorded as `(clientId, name)` pairs. `clientId` is the client's package family
  name when it has package identity, else the signed publisher + exe path.
  *(Implemented: per-user scope. Per-client scope is specified but not yet written by anything.)*
- The consent UI is rendered by a broker helper process on the interactive desktop, never by the
  requesting AI client (a client rendering its own consent screen could trivially self-approve).
  The helper is located next to the broker binary, never through `PATH`, and takes its answer
  back as an exit code. Every string it displays comes from the card on disk or from the OS —
  the requesting process is named by reading its PID, not by asking it — so a server cannot
  supply text that makes it look official.
- A prompt that is dismissed, times out, or cannot be shown is **not** a decision: nothing is
  recorded and the activation fails. Only an explicit answer is stored, and a `denied` record is
  never re-asked, because a prompt that reappears on every attempt teaches users to click
  through it.
- Prompts are serialized machine-wide. Two clients racing to activate the same server produce
  one dialog: the second re-reads the store after the first finishes and finds the answer there.
- `low`-tier and unsigned-binary cards get a visually distinct warning prompt.

## 5. Client-side integrity levels

The broker pipe accepts connections from the whole user session, including low-IL and
AppContainer clients — *discovery* is open. Policy applies at activation:

- Default policy: low-IL/AppContainer clients may activate only servers the user has already
  granted for that specific client (`scope: client`); the consent UI is never triggered *by* a
  low-trust client (prevents prompt-spam social engineering from sandboxed code).
  *(Implemented: the broker reads the connecting process's token integrity level and refuses to
  raise a prompt below Medium. An unreadable token is not treated as low integrity — a protected
  or already-exited process reads the same way, and denying on ambiguity would break ordinary
  clients to guard against a case the label cannot distinguish anyway.)*
- The per-activation connection pipe is ACL'd to the requesting client's token, so one client's
  activated session cannot be hijacked by another process reading the same pipe name.

*(Implemented.)* Both pipes carry an explicit descriptor, and the two answer opposite questions:

| Pipe | DACL | Mandatory label |
|---|---|---|
| broker `\.\pipe\mcp-locatorroker1` | this user, SYSTEM, Administrators | **Low** |
| relay `…\conn\<grantId>` | this user only | the **client's own** level |

The broker pipe's label has to be lowered deliberately. A pipe created with the default
descriptor inherits its creator's integrity level, and the mandatory policy then refuses
low-integrity clients before the DACL is consulted at all — which would make discovery, the one
thing this section says is open to sandboxes, impossible.

The relay pipes go the other way: each carries one client's live session, so labelling it at
that client's level is what stops a lower-integrity process on the same account from writing to
a grant it does not hold. Administrators are deliberately absent from a relay DACL — an
administrator can take ownership regardless, so granting it buys nothing and only makes the
descriptor a less honest statement of who the pipe is for.

If a descriptor cannot be built the code falls back to the platform default, which carries the
broker's own label: tighter than what was asked for, never looser. On unix the relay socket is
chmod 0600, since there are no integrity levels and the umask is not a guarantee.

## 6. Registry directory ACLs

- `system` tier: writable by Administrators only (standard `%ProgramData%` subdir with explicit
  ACL — note `%ProgramData%` default ACLs allow user-created files; the installer must set the
  restrictive ACL on the `servers` directory explicitly).
- `low` tier: `AppData\LocalLow` carries the low-IL mandatory label by default; the broker sets
  it explicitly anyway, so the intent is legible rather than incidental.
- State directory: broker-owned; on Windows the broker labels it Medium, so a low-integrity
  process cannot edit consent or runtime state. This is what makes "sandboxed code may register
  but never consent" true rather than merely intended.

Applied by `mcp-locator-broker secure-dirs` (`--machine` for the system tier, which the MSI runs
elevated; the per-user half runs on every `serve`). Keeping the rule in the broker rather than in
the installer means it holds however a machine was set up, and can be re-run to check or repair
one. The commands use well-known SIDs rather than account names, because `Administrators` and
`Users` are localized and this has to work on a machine in any language.

The resulting system-tier ACL, which is worth verifying after any install because the bootstrap
rests on it:

```
NT AUTHORITY\SYSTEM:(OI)(CI)(F)
BUILTIN\Administrators:(OI)(CI)(F)
NT AUTHORITY\Authenticated Users:(OI)(CI)(RX)
APPLICATION PACKAGE AUTHORITY\ALL APPLICATION PACKAGES:(OI)(CI)(RX)
```

Inheritance is cut (`/inheritance:r`). That is the load-bearing flag: `%ProgramData%` grants
users the right to create things, so without cutting inheritance a directory beneath it can end
up writable by exactly the account it is meant to be protected from.

## 7. Audit

Every activation, consent decision, supersede, and forced deactivation is appended to
`audit.log` as one JSON line: timestamp, client identity, server name, card `launchHash`,
outcome. The log is the answer to "what ran, when, and who approved it" — the same property the
Windows ODR advertises, available here down-level and cross-platform.

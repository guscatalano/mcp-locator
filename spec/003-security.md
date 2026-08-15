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

## 4. Consent

Stored in `consent.json` in the broker state directory; written only by the broker, readable by
the session. One record per server `name`:

```jsonc
{
  "com.contoso.slack": {
    "state": "granted",              // granted | denied
    "grantedAt": "2026-08-15T17:02:11Z",
    "launchHash": "sha256:…",        // hash of the card's canonicalized `local.launch` + `endpoint`
    "scope": "user"                  // "user" = all clients; "client" records add clientId
  }
}
```

- **Consent binds to `launchHash`.** If the card's launch stanza or endpoint changes, existing
  consent becomes `stale` and the user is re-prompted with a diff ("this server's launch command
  changed"). This is the rule that stops a benign registered card from being silently swapped
  for `cmd.exe /c …` after consent was given. Version bumps that don't touch launch/endpoint do
  not invalidate consent.
- Default scope is per-user ("allow for all AI clients"); the consent UI offers per-client
  restriction, recorded as `(clientId, name)` pairs. `clientId` is the client's package family
  name when it has package identity, else the signed publisher + exe path.
- The consent UI is rendered by a broker helper process on the interactive desktop, never by the
  requesting AI client (a client rendering its own consent screen could trivially self-approve).
- `low`-tier and unsigned-binary cards get a visually distinct warning prompt.

## 5. Client-side integrity levels

The broker pipe accepts connections from the whole user session, including low-IL and
AppContainer clients — *discovery* is open. Policy applies at activation:

- Default policy: low-IL/AppContainer clients may activate only servers the user has already
  granted for that specific client (`scope: client`); the consent UI is never triggered *by* a
  low-trust client (prevents prompt-spam social engineering from sandboxed code).
- The per-activation connection pipe is ACL'd to the requesting client's token, so one client's
  activated session cannot be hijacked by another process reading the same pipe name.

## 6. Registry directory ACLs

- `system` tier: writable by Administrators only (standard `%ProgramData%` subdir with explicit
  ACL — note `%ProgramData%` default ACLs allow user-created files; the installer must set the
  restrictive ACL on the `servers` directory explicitly).
- `low` tier: `AppData\LocalLow` carries the low-IL mandatory label by default; nothing to do.
- State directory: broker-owned; on Windows the broker sets a deny-write ACE for the
  low-integrity label so sandboxed processes cannot edit consent or runtime state.

## 7. Audit

Every activation, consent decision, supersede, and forced deactivation is appended to
`audit.log` as one JSON line: timestamp, client identity, server name, card `launchHash`,
outcome. The log is the answer to "what ran, when, and who approved it" — the same property the
Windows ODR advertises, available here down-level and cross-platform.

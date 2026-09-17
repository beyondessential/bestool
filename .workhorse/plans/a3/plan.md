# Ask Canopy for names and certificates

Client side of Canopy-issued DNS and TLS. Behaviour is specified in [TLS](../../specs/canopy/certificates.md), [TLSD](../../specs/canopy/certificate-delivery.md) and [NAM](../../specs/canopy/names.md), with the two healthchecks in [CHK-CCO](../../specs/canopy/certificate-collection-check.md) and [CHK-CCT](../../specs/tamanu/caddy-certs.md); this plan holds the design reasoning and the outstanding work.

Linux lands here. The Windows-specific parts are carried by their own card. Hardening the key store's unlock key to a TPM is likewise its own card, since it improves the device mTLS key equally.

## How Caddy behaves, measured

Established against Caddy 2.11.4 with a stub certificate endpoint and a stub ACME directory, rather than reasoned from its source. The source alone is misleading here: `automaticHTTPSPhase1` never inspects connection policies for a certificate manager, which suggests a manager does not suppress Caddy's own issuance — but the Caddyfile adapter puts `get_certificate` in the *automation* policy, where it does.

With a per-site `tls { get_certificate http … }` block and the site's issuer left in place:

| Endpoint answers | Caddy does |
|---|---|
| a chain | serves it, orders nothing of its own (zero ACME traffic) |
| `204` decline | falls back to its configured issuer, obtains normally |
| an error, or unreachable | handshake fails; no fallback, even for a name it could issue |
| — (Caddy already holds its own cert) | serves that when the endpoint errors |
| — (name not in any site config) | never asks; no issuance by either party |

Two consequences drive the design.

**The endpoint must decline, not error.** An error takes down a name Caddy would otherwise have covered. Every "no chain for you" case is a `204`. Only the peer-check refusal errors, deliberately.

**`auto_https disable_certs` is the wrong lever.** It does stop Caddy issuing while leaving the manager working, but it removes the fallback that makes this deployable before the Route 53 credential is withdrawn.

A catch-all manager policy was considered and rejected. It lets an unconfigured SNI reach the endpoint, but a decline then falls through to on-demand issuance, which `auto_https disable_certs` does *not* cover — only a refusing `on_demand_tls ask` endpoint stops it. Failed on-demand attempts retry every 60 seconds for up to 30 days per name, so an arbitrary SNI becomes a month of pointless orders. Per-site blocks avoid the area entirely, at the cost of not learning a name Caddy is not configured for, which is a name this card would not certify anyway.

## Where things live

The registration store is the precedent for the key store: a machine-bound encrypted file under `/etc/bestool`, blake3-derived machine-id passphrase over age/scrypt, with a low scrypt work factor so the arena stays inside the daemon's `MemoryMax`. It caches in-process so repeated reads do not re-run scrypt, which matters more here — the delivery handler is on the handshake path and must not unlock anything.

One encrypted file holds every name's key; collected chains sit beside it in the clear. This keeps the encrypted payload small and static: a collection rewrites a plain file rather than re-encrypting the store, and keys are unlocked only when one is generated or read back after a restart.

## Identifying the caller

Peer credentials over a socket are not reachable through Caddy. `HTTPCertGetter` parses the URL with `url.Parse` and issues through `http.DefaultClient`: no unix socket, no custom dialer, no client certificate, no configurable transport. So the connection is ordinary loopback TCP and the peer is resolved out of band — `/proc/net/tcp` and `/proc/net/tcp6` for the socket inode, then a scan of `/proc/*/fd` for the process holding it.

The permission model follows tailscaled's: superuser by default, plus one further user named in configuration, the same shape as `TS_PERMIT_CERT_UID=caddy`, which exists because Caddy commonly runs as its own unprivileged user.

## Scheduling

A slow tick carries the steady state; a name sitting in `pending` schedules a sooner retry until it resolves. The daemon ticks each background task at one fixed `interval()`, so both rhythms come from inside the task: tick at the fast rate and rate-limit the steady-state work, which sits better with the watchdog (each tick counts as activity) than doing its own waiting within a run.

A Canopy-issued chain's lifetime is not known ahead of time — `not_after` comes back on the response — so the slow rate must suit the shortest profile the authority might use.

## Known costs

A host can sit quietly on Caddy's own issuance, Route 53 credential still in use, and look healthy. This is why `caddy_certs` has to distinguish a chain the daemon served from one Caddy issued.

An endpoint that errors takes the name down rather than handing it back, so the daemon being down is an outage for every Canopy-served name Caddy holds no certificate of its own for. Answering from memory keeps the window to a process restart, but the window is real.

Taking the union of a machine's applications drops the boundary Canopy draws between workloads on one box. Nothing on this side knows which application a Caddy site belongs to, and every machine is single-application today, so the union costs nothing yet.

The day a machine hosts two workloads, the union over-reaches rather than silently misbehaving: Canopy resolves the application from the *name*, refuses a name no application on the machine declares, and applies the declaring application's own grant and pause. So the failure mode is a refusal the agent reports, not a certificate issued under the wrong workload's authority. Two consequences to live with: on a multi-application machine an undeclared name has to be declared by an operator in Canopy before the agent can register or certify it, and a union that reads "entitled" can still be refused per name.

## Build

- [ ] Key store: generate P-256 keys, one per name, in a machine-bound encrypted file beside the registration; chains as plain files alongside.
- [ ] CSR generation, base64 DER, exactly one name per request.
- [ ] Entitlements call, including the union over the applications list, the domain check, and the paused and grant states.
- [ ] Read Caddy's admin config for active subjects; reuse what `caddy_certs` already does rather than parsing a Caddyfile.
- [ ] Request/collect against Canopy, holding pending orders and retrying them sooner than the steady tick.
- [ ] Revocation and forced key replacement handling, including that a revocation pauses the server in Canopy so the replacement request is refused until an operator lifts it.
- [ ] Background task in the alertd daemon wiring the above, with the fast-tick-and-rate-limit shape.
- [ ] Certificate endpoint: in-memory answers only, decline by default, chain plus key as PEM on a hit.
- [ ] Peer identification and the configured permitted user; refusal as a failure.
- [ ] Task HTTP endpoints for status and for forcing a collection.
- [ ] `bestool canopy certs`: list, request, collect.
- [ ] `bestool canopy dns`: register, withdraw, show.
- [ ] `canopy_certificates` healthcheck, including its skip conditions.
- [ ] Teach `caddy_certs` about a chain served by the daemon.
- [ ] Document the required Caddyfile shape for the deployment to apply.

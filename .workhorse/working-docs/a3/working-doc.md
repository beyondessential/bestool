---
status: complete
---

# Ask Canopy for names and certificates

The client side of Canopy-issued DNS and TLS: a bestool server generates its own key, registers its name, and collects a certificate from Canopy on a timer, replacing per-server Route 53 access.

## Shape

The flow lives as a background task in the alertd daemon, ticking on its own interval alongside the existing Canopy mTLS renewal.
Caddy gets the chain through `get_certificate http` pointed at an alertd endpoint, so a collected renewal is served without a Caddy reload.
The task exposes HTTP endpoints for the CLI to force a run and to report status, using the `/tasks/{task}/{endpoint}` mounting the daemon already does.

Canopy does not replace Caddy's own issuance, it takes precedence over it.
A site keeps its existing issuer, and Caddy asks alertd first: when alertd has a chain it is served and Caddy orders nothing, and when alertd says it has none Caddy issues for itself exactly as it does today.
So a host can run this before its Route 53 credential is withdrawn, and the credential becomes removable once Canopy is reliably answering rather than as a precondition for trying.

The DNS half is deliberately narrower than the certificate half on this card: publishing addresses is driven only by an explicit CLI call, as a proof of concept, while certificates are automated.

The Caddyfile shape a Canopy-issued host needs is an interface contract this card states, and the deployment applies.
bestool does not write or patch the Caddyfile.

Linux lands on this card.
Windows is designed for but not built here, and its specific parts are carried by their own card rather than left silently absent.

## Behaviour

### What the server may do

The task asks Canopy what this server is entitled to rather than remembering it, because a grant can be withdrawn.
The answer carries the domains the server's group controls, whether it may manage DNS and TLS separately, whether Canopy is currently paused for this server, the names it has registered addresses for, and the certificates Canopy holds for it.
A server with no grants, or whose group controls no domain, gets an empty answer rather than an error, so asking is never itself a failure.

Canopy answers a single-application machine in the flat fields and a machine hosting several in an applications list, each entry with its own domains, grants, and paused state.
The task acts on the union of what it is told: the flat fields when that is the answer, and every application entry's domains and grants together when it is not.
A name is actionable if any application on the machine could act on it, and the task does not try to work out which workload a name belongs to.

A name is only acted on when it sits at or beneath one of the domains the group controls.

### Which names get certified

Caddy's active subjects are the source of names: the task reads the site addresses Caddy is configured to serve from its admin config and orders for those, ahead of any client arriving.
The `caddy_certs` check already reads exactly this config and derives its active subject set the same way.

A handshake ask is a backstop between polls rather than a discovery route of its own.
Caddy asks alertd only for names it is already configured to serve, so an ask never brings news of a name the poll would not also find; what it adds is immediacy, when a name is added to Caddy's config and a client arrives before the next poll.
An ask for an in-domain name with nothing held records the name so an order follows.

An explicit call is the third way in, for operator convenience and pre-provisioning.
It is not a pathway the system depends on in normal running.

### Holding the key

The private key is generated on the machine and never leaves it.
Canopy signs what it is given: the request carries a CSR only, DER base64, asking for exactly one name, and a CSR naming anything else is refused rather than trimmed.
Each name has its own key and its own CSR, so a certificate whose key Canopy condemns costs only that name a replacement.

The key must survive an alertd restart, since Canopy keys what it holds by name and key, and a lost key turns a collection into a fresh order.
Keys are ECDSA over P-256, which is what the device mTLS key already uses, and which the authority behind Canopy issues against.

Keys are held in a machine-bound encrypted store, the same shape as the one holding the device mTLS key: the file is keyed by a passphrase derived from the host's machine id, so no private key is at rest in plaintext and a cloned disk cannot reuse what it carries.
One file holds every name's key.
Collected chains sit beside it as plain files, because a chain is public and is the thing that changes most often.

### Requesting and collecting

Request and collect are the same call, and it is safe to repeat.
A name and key Canopy already holds a certificate for is answered from what it holds rather than ordered again, so a server that lost its local copy costs the authority nothing.

The response carries the state (`pending`, `issued`, `failed`, `revoked`), the chain once there is one, when it expires, whether it is usable, whether it was revoked, whether the key must be replaced, and the last error while Canopy is still retrying.

Collection is on a timer, never while a client waits.
A server is expected to hold a certificate before it needs one.

### Serving the chain

Caddy asks alertd for a certificate during the handshake, passing the SNI as `server_name` alongside the signature schemes, cipher suites, and the local IP the client reached.
Caddy asks before consulting anything it holds itself, so a chain alertd answers with is the one served.

An ask for a name alertd holds a usable chain for is answered with the PEM chain and private key together.

Every other ask is declined, and declining is distinct from failing.
A decline hands the handshake back to Caddy, which then issues for the name itself as it would if alertd were not configured at all.
A failure does not: Caddy treats an error from the endpoint as "could have served this and did not", abandons the handshake, and does not fall back to issuing its own.
So the endpoint declines whenever it has no chain to offer — a name outside the group's domains, a name with nothing collected yet, a chain that has been revoked — and reserves failing for when it genuinely cannot answer.

Every ask is answered from what the daemon already holds in memory.
The handler reaches no network, unlocks no key store, and touches no disk, because it sits on the TLS handshake path: Caddy's manager issues the request through a client with no timeout, using the provisioning context rather than the per-handshake one, so a handler that blocks stalls the handshake indefinitely.
Answering from memory is also what keeps a decline cheap enough to be the default answer.

### Renewal

Canopy decides when a certificate is due and re-orders on its own; this side's part is to keep collecting.
The chain in hand stays valid while a renewal is under way, so a renewal in flight is not an outage and is not treated as one.
Because Caddy asks for the certificate on each handshake rather than holding a file, a renewal that lands is served without a reload.

### The awkward states

A revoked certificate is stopped being served and asked for again.
A certificate whose key must be replaced needs a new key generated first, not just a new request.
A last error is surfaced rather than spun on.
While Canopy reports the server paused, it is making no new changes on the server's behalf and requests are refused, so the task waits rather than retrying.

In each of these the endpoint declines rather than fails, so a name Canopy cannot currently serve falls to Caddy's own issuance instead of going dark.

### When the grant is gone

A server that may not obtain certificates stops asking, and stops quietly.

The task draws no distinction between a grant that was withdrawn and one that was never held, and keeps no record that it once had one.
Withdrawing a grant is a containment action taken while an incident is under way, most likely on a host that is compromised.
That makes the host the wrong place to reason about it: what it remembers is in an attacker's hands, and what it reports lands on people already working the incident.
So the task asks entitlements, acts on the answer, and holds no history of its own authorisation.

Chains already collected keep being served.
A withdrawn grant is not a revocation — Canopy says separately when a certificate must stop being served, and that is the lever which takes a chain out of service.

### Being watched

Two checks cover this, because a chain that stopped being collected is a failure nothing currently catches.

A new check grades the collection pipeline itself: the names wanted against the chains held, how long until the earliest expires, and any last error Canopy reported.

It skips when the server holds no TLS grant, or while Canopy reports it paused, naming the unmet precondition.
A pause is never escalated however long it lasts: Canopy set it, so reporting it back tells the authority what it already knows.
Both states stay visible on the task's status endpoint and through the CLI, for whoever is looking at the box.
A skip neither degrades the sweep nor triggers a heal, and a skipped result closes any issue the check had already opened, so a grant withdrawn mid-incident silences this check rather than adding to the incident.
The reason a skip carries says only that the server may not obtain certificates, without speculating why.

`caddy_certs` keeps grading what Caddy serves, taught to recognise a chain that came from alertd rather than Caddy's own store so it neither mis-grades it nor ignores it.
It is also the check that notices a host quietly running on Caddy's own issuance because alertd has been declining every name.

### CLI surface

The CLI reaches the running daemon over its HTTP endpoints, the way the existing Canopy commands already find it.

Certificates, under `bestool canopy certs`: report what Canopy holds for this server, read from entitlements; request a name explicitly, for pre-provisioning; force a collection run rather than waiting for the tick.

DNS, under `bestool canopy dns`: register a name's addresses, withdraw a name, and report what Canopy currently has registered for this server and whether the zone has caught up.
This is the proof-of-concept path, driven by arguments with nothing persisted and nothing re-registering on its own.

## Implementation options

### Where the key and chain live

Canopy keys what it holds by name and key fingerprint, so a key has to outlive an alertd restart or every restart becomes a fresh order.
The registration store is the neighbour to follow: it already holds the device mTLS key in a machine-bound encrypted file under `/etc/bestool`, keyed by a blake3-derived machine-id passphrase over age/scrypt, with a low work factor chosen so the arena stays inside the daemon's `MemoryMax`.
It caches in-process precisely so repeated reads don't re-run scrypt, which matters more here — the delivery handler is on the handshake path and must not be unlocking anything.

Following that precedent, one encrypted file holds every name's key and the collected chains sit beside it in the clear.
This keeps the encrypted payload small and static: a collection that lands rewrites a plain file rather than re-encrypting the key store, and the keys are only unlocked when one is generated or read back after a restart.

Hardening the unlock key to a TPM is worth having and is not this card's work, since it improves the device key equally.

### Protecting the endpoint that hands out the key

The endpoint hands out a private key, so the caller is identified rather than trusted for being local.

Peer credentials over a socket are not reachable through Caddy.
`HTTPCertGetter` parses the configured URL with `url.Parse` and issues the request through `http.DefaultClient`: no unix socket, no custom dialer, no client certificate, no configurable transport at all.
So the connection alertd sees is an ordinary loopback TCP one, and the peer has to be resolved out of band — the connection to its owning process, and that process to its user.
On Linux that is `/proc/net/tcp` and `/proc/net/tcp6` to get the socket inode, then a scan of `/proc/*/fd` to find the process holding it.

The permission model follows tailscaled's.
By default only root may fetch a certificate, and alertd is configured with the name or id of a further user permitted to — the same shape as `TS_PERMIT_CERT_UID=caddy`, which exists because Caddy commonly runs as its own unprivileged user.

A caller that fails this check is refused, and a refusal is a failure rather than a decline, because it is a misconfiguration to be fixed and not a name Caddy should quietly start issuing for itself.

Windows needs its own lookup (`GetExtendedTcpTable`) and its own user model, and may fall back to trusting loopback if that proves disproportionate.
That decision belongs to the Windows card rather than being inherited by default here.

### Running alongside Caddy's own issuance

The behaviour was established against Caddy 2.11.4 with a stub certificate endpoint and a stub ACME directory, rather than reasoned from the source.

With a per-site `tls { get_certificate http … }` block and the site's issuer left in place:

- When the endpoint answers with a chain, Caddy serves it and orders nothing of its own. No ACME traffic at all.
- When the endpoint declines with `204`, Caddy falls back to its configured issuer and obtains normally.
- When the endpoint errors or is unreachable, the handshake fails. Caddy does not fall back, even for a name it could have issued for itself.
- When Caddy independently holds a certificate for the name, from an earlier fallback, it serves that when the endpoint errors.
- A name the site is not configured for never reaches the endpoint, and triggers no issuance.

So the contract is a per-site manager pointed at alertd, with nothing else changed.
`auto_https disable_certs` is the wrong lever here: it does stop Caddy issuing while leaving the manager working, but it removes the fallback that makes this safe to deploy incrementally.

The third point is the one that shapes the handler: a chain alertd cannot serve must come back as a decline, because an error takes down a name Caddy would otherwise have covered.

A catch-all manager policy is a different configuration and a worse one.
It does let an unconfigured SNI reach the endpoint, but a decline then falls through to on-demand issuance, and on-demand is not covered by `auto_https disable_certs` — only a refusing `on_demand_tls ask` endpoint stops it.
Failed on-demand attempts retry every 60 seconds for up to 30 days per name, so an arbitrary SNI becomes a month of pointless orders.
Sticking to per-site blocks avoids the whole area, and costs only the ability to learn a name Caddy is not configured for, which is a name this card would not certify anyway.

### Collection interval

A slow tick carries the steady state, and a name sitting in `pending` schedules its own sooner retry until it resolves.
Normal running stays quiet; an order in flight is collected promptly rather than waiting out a full interval.

The daemon ticks each background task at one fixed `interval()`, so the two rhythms have to come from inside the task.
Either it ticks at the fast rate and rate-limits its own steady-state work, or it does its own waiting within a run — and the first sits better with the watchdog, which counts each tick as activity.

A Canopy-issued chain's lifetime is not known to this side ahead of time; `not_after` comes back on the response, so the slow rate still has to suit the shortest profile the authority might use.

## Open questions

None outstanding.

## Trade-offs

Running Canopy in front of Caddy's own issuance rather than instead of it means a host is never worse off than it is today.
Canopy answering is a strict improvement; Canopy silent leaves the existing path working.
The cost is that a host can sit quietly on its own issuance, with the Route 53 credential still in use and nobody the wiser, which is why the checks have to notice a host that is declining every name rather than only a host that is failing.

Serving through `get_certificate` rather than files plus a reload removes the whole install-and-reload step.
It buys a handshake-time dependency on alertd, and that dependency is sharper than it first looks: an endpoint that errors takes the name down rather than handing it back, so alertd being down is an outage for every name Canopy serves that Caddy holds no certificate of its own for.
Answering from memory keeps the window to the length of a process restart, but the window is real.

The handshake ask is a backstop, not a discovery route.
Caddy only asks for names it is configured to serve, which the active-subjects poll already reads from the same admin config, so the ask adds immediacy between polls and nothing else.

Driving DNS registration only from an explicit CLI call keeps publishing records a deliberate act.
Publishing addresses points real traffic at this box, which is not something a poll or a stray handshake should be able to cause.

Treating a withdrawn grant as indistinguishable from one never held gives up the ability to alert on a capability disappearing.
That is the point: the case it would alert on is one where an operator has already acted, and where this host is a suspect rather than a witness.
The cost is that a grant withdrawn by mistake goes unremarked here, and has to be noticed from Canopy's side.

Taking the union of a machine's applications rather than mapping each name to one drops the boundary Canopy is drawing between workloads on the same box.
Nothing on this side currently knows which application a Caddy site belongs to, and every machine is single-application today, so the union costs nothing yet and is the wrong answer the day a machine hosts two workloads with different grants.

## Testing notes

### End to end

- A server with the TLS grant, on a name inside its group's domains, obtains a certificate from Canopy and Caddy serves it.
- A server issuing this way needs no DNS credential of its own.

### Precedence and fallback

- With a chain held, Caddy serves Canopy's and places no order of its own.
- With no chain held, the endpoint declines and Caddy issues for the name itself.
- A name that falls back and a name served from Canopy can coexist on one host.
- The endpoint declines rather than errors for every case where it has no chain: out-of-domain, not yet collected, revoked, no grant, paused.
- A caller that fails the peer check is refused with an error, not a decline.

### Keys and requests

- The private key was generated on the machine and appears in no request body; the request carries a CSR only.
- A CSR naming more than the one name is refused by Canopy rather than trimmed.
- Each name gets its own key, so replacing one name's key leaves the others' certificates untouched.
- Keys survive an alertd restart: a restart collects what was already ordered rather than ordering afresh.

### Ordering and collection

- A first request answers `pending` and a later collection lands the chain.
- A name sitting in `pending` is retried sooner than the slow tick, and stops being retried once it resolves.
- A repeated request for a name and key Canopy already holds is answered from what it holds rather than ordering again.
- A name appearing in Caddy's active subjects is ordered before any client arrives.
- A handshake for a configured in-domain name with nothing held records the name and an order follows.
- A renewal lands without the served chain lapsing, and is served without a Caddy reload.

### Refusals and awkward states

- A revoked certificate stops being served and is asked for again.
- A certificate reporting that the key must be replaced gets a new key before the next request.
- Entitlements reporting the server paused stops the task making new requests.
- A `last_error` from Canopy is surfaced rather than retried into silently.
- A machine answered with an applications list acts on the union of their domains and grants.
- A server whose TLS grant is withdrawn stops asking, keeps serving the chains it holds, and raises nothing.
- A grant withdrawn while the check had an issue open closes that issue rather than adding to it.
- A server that never held a grant and one that has lost it are indistinguishable in what the host records and reports.

### The delivery endpoint

- A held chain is still served while Canopy is unreachable, because the handler answers from memory.
- The handler does no I/O on the handshake path, so a stalled Canopy call cannot stall a handshake.

### CLI

- `bestool canopy certs` reports what Canopy holds for this server, requests a name explicitly, and forces a collection run.
- `bestool canopy dns` registers a name's addresses, withdraws a name, and reports what Canopy has registered and whether the zone has caught up.

### Being watched

- The new check fails when a name is wanted and no chain has been collected for it.
- A host serving every name from Caddy's own issuance, because alertd declines them all, is noticed rather than looking healthy.
- `caddy_certs` neither mis-grades nor ignores a chain that came from alertd rather than Caddy's own store.

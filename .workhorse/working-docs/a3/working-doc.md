---
status: draft
---

# Ask Canopy for names and certificates

The client side of Canopy-issued DNS and TLS: a bestool server generates its own key, registers its name, and collects a certificate from Canopy on a timer, replacing per-server Route 53 access.

## Shape

The flow lives as a background task in the alertd daemon, ticking on its own interval alongside the existing Canopy mTLS renewal.
Caddy gets the chain through `get_certificate http` pointed at an alertd endpoint, so a collected renewal is served without a Caddy reload.
The task exposes HTTP endpoints for the CLI to force a run and to report status, using the `/tasks/{task}/{endpoint}` mounting the daemon already does.

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
This check is what stops an arbitrary SNI arriving from the internet from originating an order.

### Which names get certified

Three things may originate a certificate order.

Caddy's active subjects are the primary source: the task reads the site addresses Caddy is configured to serve from its admin config and orders ahead of any client arriving.
The `caddy_certs` check already reads exactly this config and derives its active subject set the same way.

A handshake ask is the second primary source: an ask for a name alertd holds no certificate for records the name and starts an order.
That handshake cannot be answered, because a first request to Canopy records the order and answers `pending`; later handshakes succeed once the chain has been collected.

An explicit call is the third, for operator convenience and pre-provisioning only.
It is not a pathway the system depends on in normal running.

### Holding the key

The private key is generated on the machine and never leaves it.
Canopy signs what it is given: the request carries a CSR only, DER base64, asking for exactly one name, and a CSR naming anything else is refused rather than trimmed.
Each name has its own key and its own CSR, so a certificate whose key Canopy condemns costs only that name a replacement.

The key must survive an alertd restart, since Canopy keys what it holds by name and key, and a lost key turns a collection into a fresh order.
Keys are ECDSA over P-256, which is what the device mTLS key already uses.

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

An ask for a name alertd holds a usable chain for is answered with the PEM chain and private key together, and Caddy serves it.
An ask for a name outside the group's domains is answered `204`, which Caddy reads as "not managing a certificate for this handshake" and falls through to its other certificate sources.
An ask for an in-domain name with nothing collected yet records the name for ordering, and cannot be answered.

Every ask is answered from what the daemon already holds in memory.
The handler reaches no network, unlocks no key store, and touches no disk, because it sits on the TLS handshake path: Caddy's manager issues the request through a client with no timeout, using the provisioning context rather than the per-handshake one, so a handler that blocks stalls the handshake indefinitely.

### Renewal

Canopy decides when a certificate is due and re-orders on its own; this side's part is to keep collecting.
The chain in hand stays valid while a renewal is under way, so a renewal in flight is not an outage and is not treated as one.
Because Caddy asks for the certificate on each handshake rather than holding a file, a renewal that lands is served without a reload.

### The awkward states

A revoked certificate is stopped being served and asked for again.
A certificate whose key must be replaced needs a new key generated first, not just a new request.
A last error is surfaced rather than spun on.
While Canopy reports the server paused, it is making no new changes on the server's behalf and requests are refused, so the task waits rather than retrying.

### Being watched

Two checks cover this, because a chain that stopped being collected is a failure nothing currently catches.

A new check grades the collection pipeline itself: the names wanted against the chains held, how long until the earliest expires, any last error Canopy reported, and whether the TLS grant is present or the server paused.
`caddy_certs` keeps grading what Caddy serves, taught to recognise a chain that came from alertd rather than Caddy's own store so it neither mis-grades it nor ignores it.

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

Windows needs its own lookup (`GetExtendedTcpTable`) and its own user model, and may fall back to trusting loopback if that proves disproportionate.
That decision belongs to the Windows card rather than being inherited by default here.

### Stopping Caddy issuing on its own

Configuring `get_certificate` on a site does **not** stop Caddy obtaining its own ACME certificate for that site's names.
A certificate manager is a connection-policy concern, and the managed set is decided separately in `automaticHTTPSPhase1`, which excludes a name only via `SkipCerts`, `DisableCerts`, a certificate already loaded, a name that does not qualify, or an existing explicit automation policy.
Nothing there inspects the connection policy for a manager.
So a site pointed at alertd still places its own order through whatever issuer it is configured with — which is the per-server DNS-01 path this card exists to delete.

The consequence is not usually a competing certificate.
DNS issuance is chosen on these hosts precisely because Caddy cannot complete the other challenges, so once the Route 53 credential is gone Caddy's own attempt fails rather than succeeds.
What it leaves is a permanently failing issuance loop: retries, log noise, and authority rate limits spent on orders that can never complete.
Where the HTTP or TLS-ALPN challenges would work, Caddy could still succeed on its own, and then disabling management is what actually keeps issuance on Canopy.

Either way the Caddyfile has to disable Caddy's own certificate management as well as point at alertd, most directly with the global `auto_https disable_certs`, which leaves the HTTP-to-HTTPS redirects in place.

Separately, `get_certificate` implies `on_demand` is enabled.
Answering `204` for an out-of-domain name hands the handshake back to Caddy, and on-demand is the path that reaches issuance for a name not in the config.
Whether that matters once certificate management is disabled outright needs confirming against a real Caddy; if it does, an `on_demand_tls ask` endpoint constrains it, and alertd is the natural place for that too.

### Collection interval

A slow tick carries the steady state, and a name sitting in `pending` schedules its own sooner retry until it resolves.
Normal running stays quiet; an order in flight is collected promptly rather than waiting out a full interval.

The daemon ticks each background task at one fixed `interval()`, so the two rhythms have to come from inside the task.
Either it ticks at the fast rate and rate-limits its own steady-state work, or it does its own waiting within a run — and the first sits better with the watchdog, which counts each tick as activity.

A Canopy-issued chain's lifetime is not known to this side ahead of time; `not_after` comes back on the response, so the slow rate still has to suit the shortest profile the authority might use.

## Open questions

- [ ] Whether alertd also serves an `on_demand_tls ask` endpoint to stop Caddy attempting its own issuance for arbitrary SNI, and whether on-demand is still reachable once `auto_https disable_certs` is set. Needs confirming against a real Caddy rather than reasoning.
- [ ] Whether Canopy's authority constrains the key algorithm; P-256 is chosen on this side, and a refusal would be found late.
- [ ] How long the task waits while Canopy reports the server paused, and how a withdrawn TLS grant reads on the new check — a server that was never granted one and a server that lost one look the same from entitlements alone.

## Trade-offs

Serving through `get_certificate` rather than files plus a reload removes the whole install-and-reload step, and buys a handshake-time dependency on alertd in exchange.
If alertd is down, nothing is served, where a file on disk would keep working.
That is the cost of never needing a reload.

Learning names from handshake asks means a genuinely new name is briefly unserved, since its first ask cannot be answered.
Polling Caddy's active subjects covers the normal case ahead of time, so the handshake path is the backstop for a name that appears between polls rather than the usual route.

Driving DNS registration only from an explicit CLI call keeps publishing records a deliberate act.
Publishing addresses points real traffic at this box, which is not something a poll or a stray handshake should be able to cause.

Taking the union of a machine's applications rather than mapping each name to one drops the boundary Canopy is drawing between workloads on the same box.
Nothing on this side currently knows which application a Caddy site belongs to, and every machine is single-application today, so the union costs nothing yet and is the wrong answer the day a machine hosts two workloads with different grants.

## Testing notes

### End to end

- A server with the TLS grant, on a name inside its group's domains, obtains a certificate from Canopy and Caddy serves it.
- A server issuing this way needs no DNS credential of its own.

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
- A handshake for an in-domain name with nothing held records the name and an order follows.
- A renewal lands without the served chain lapsing, and is served without a Caddy reload.

### Refusals and awkward states

- An SNI outside the group's domains is answered `204` and originates no order.
- A revoked certificate stops being served and is asked for again.
- A certificate reporting that the key must be replaced gets a new key before the next request.
- Entitlements reporting the server paused stops the task making new requests.
- A `last_error` from Canopy is surfaced rather than retried into silently.
- A machine answered with an applications list acts on the union of their domains and grants.

### The delivery endpoint

- A caller that is not the permitted user is refused a certificate.
- A held chain is still served while Canopy is unreachable, because the handler answers from memory.
- The handler does no I/O on the handshake path, so a stalled Canopy call cannot stall a handshake.

### CLI

- `bestool canopy certs` reports what Canopy holds for this server, requests a name explicitly, and forces a collection run.
- `bestool canopy dns` registers a name's addresses, withdraws a name, and reports what Canopy has registered and whether the zone has caught up.

### Being watched

- The new check fails when a name is wanted and no chain has been collected for it.
- `caddy_certs` neither mis-grades nor ignores a chain that came from alertd rather than Caddy's own store.

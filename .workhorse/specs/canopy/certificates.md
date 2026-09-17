---
id: TLS
---

# Canopy-issued TLS certificates

A server obtains its TLS certificates from Canopy rather than from a certificate authority it reaches itself.
Canopy holds the account with the authority and proves control of the name through DNS, so the server needs no DNS credential of its own.

The private key never leaves the machine: Canopy signs a certificate signing request and never sees the key behind it.

The alertd daemon runs the collection on a schedule and serves the collected chain to Caddy; see [TLSD](certificate-delivery.md) for how Caddy is served.
Which names a server may certify follows from its entitlement, see [NAM](names.md).
Whether the collection is working is graded by its own healthcheck, see [CHK-CCO](certificate-collection-check.md).
The server authenticates to Canopy with the device identity in its registration, see [CHK-REG](registration.md).

## Which names are certified

A name is certified when it is both a site address Caddy is configured to serve, read from Caddy's live admin configuration, and a name the server's entitlement covers: within the group's domains, with the TLS grant held and no pause in force ([NAM](names.md)).
Caddy's configuration says which names the host answers on, and the entitlement says which of those Canopy will act on; a name meeting one test and not the other is left to Caddy's own issuance.
The daemon orders for the names meeting both ahead of any client arriving, because a certificate is obtained before it is needed rather than while a client waits.

A handshake for a configured name the daemon holds no chain for records that name so that an order follows, which covers a name added to Caddy between reads of its configuration.
A name recorded this way is subject to the same entitlement test as one read from the configuration, so a handshake cannot conjure an order for a name outside the server's reach.

A name may also be requested explicitly through a command, for pre-provisioning.

## Keys

Each name has its own key pair, generated on the machine, and its own certificate signing request.
A key covering one name means a key Canopy condemns costs only that name a replacement.

Keys are ECDSA over the P-256 curve.

A request carries the signing request as base64-encoded DER and names exactly one name.
Canopy refuses a request whose signing request carries any other name rather than trimming it.

Keys are held in a machine-bound encrypted store alongside the device identity, keyed by a passphrase derived from the host's machine id, so no private key is at rest in plaintext and the store cannot be read on a different machine.
One store holds every name's key.
Collected chains are held beside it in the clear, a chain being public, so a collection that lands rewrites a plain file rather than the encrypted store.

Keys outlive a daemon restart, so a restart collects an order already placed rather than placing a new one.

## Requesting and collecting

A request and a collection are the same call to Canopy, and it is safe to repeat.
Proving control of a name through DNS takes longer than a client waits, so the first call records the order and answers that it is pending, and a later call collects the chain.

Canopy answers from a certificate it already holds for the same name and key rather than ordering again, so a server that has lost its local copy of a chain costs the authority nothing.

Canopy's answer carries the state of the order, the chain once there is one, when it expires, whether it can be served, whether it has been revoked, whether the key must be replaced, and the reason the last attempt failed while Canopy is still retrying.
A reported error is surfaced rather than retried into.

The entitlement answer lists the certificates Canopy holds, with the name and key each covers and when it expires, but not the chains themselves.
It is therefore what the server reconciles against — which names Canopy already has a certificate for, and whether that certificate covers a key the server still holds — while the chain itself is only ever collected per name.

The daemon collects on a repeating schedule.
A name whose order is pending is retried sooner than the steady-state schedule until the order resolves.

## Renewal

Canopy decides when a certificate is due and re-orders on its own; the server's part is to keep collecting.
A chain already in hand stays valid while a renewal is under way, so a renewal in flight is not a failure and a renewed chain replaces the old one without the served certificate lapsing.

How long a chain lives is Canopy's to choose and is not known before one arrives, the expiry coming back with the chain.
So the collection schedule suits the shortest lifetime Canopy might issue under, and every judgement the server makes about a chain running out is a fraction of that chain's own lifetime rather than a fixed duration.

## Revocation and key replacement

A certificate Canopy reports as revoked stops being served immediately, and a replacement is requested.

Revoking a certificate pauses the server in Canopy, so the replacement request is refused until an operator lifts the pause.
The server therefore stops serving the revoked chain at once and keeps asking for its replacement under the ordinary schedule, rather than treating the refusal as a fault: revocation is an operator acting on this host, and the pause is that operator deciding when it may have a certificate again.
Other names the server holds chains for are unaffected and continue to be served.

A certificate Canopy reports as requiring its key to be replaced gets a new key pair before the next request, rather than a further request against the same key.
A condemned key is never certified again, for any name, so replacing it is the only way forward and the server does not wait for an operator to act on the key itself.

## When the grant is absent or the server is paused

A server that may not obtain certificates stops requesting them, and so does one Canopy reports as paused.

Chains already collected continue to be served in both cases.
Neither a withdrawn grant nor a pause is a revocation, and revocation is what takes a chain out of service; see [TLSD](certificate-delivery.md) for what the daemon serves.
So withdrawing a grant from a host under suspicion stops it obtaining anything new without also dropping every name it currently answers on.

A server holds no record of having been entitled before, so a grant that was withdrawn and a grant that was never held are indistinguishable in what it keeps and in what it reports.
Withdrawal is a containment action taken during an incident, and a host under suspicion is not where that is reasoned about.

A pause is Canopy's to lift and no length of pause is escalated from here, so a paused server waits rather than retrying against the refusal.

## Commands

`bestool canopy certs` reaches the running daemon over its HTTP interface.
It reports the certificates Canopy holds for this server, requests a name, and runs a collection without waiting for the schedule.

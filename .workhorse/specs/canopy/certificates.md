---
id: TLS
---

# Canopy-issued TLS certificates

A server obtains its TLS certificates from Canopy rather than from a certificate authority it reaches itself.
Canopy holds the account with the authority and proves control of the name through DNS, so the server needs no DNS credential of its own.

The private key never leaves the machine: Canopy signs a certificate signing request and never sees the key behind it.

The alertd daemon runs the collection on a schedule and serves the collected chain to Caddy; see [TLSD](certificate-delivery.md) for how Caddy is served.
Which names a server may certify follows from its entitlement, see [NAM](names.md).
The server authenticates to Canopy with the device identity in its registration, see [REG](registration.md).

## Which names are certified

The names a server holds certificates for are the site addresses Caddy is configured to serve, read from Caddy's live admin configuration.
The daemon orders for those names ahead of any client arriving, because a certificate is obtained before it is needed rather than while a client waits.

A handshake for a configured name the daemon holds no chain for records that name so that an order follows, which covers a name added to Caddy between reads of its configuration.

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

The daemon collects on a repeating schedule.
A name whose order is pending is retried sooner than the steady-state schedule until the order resolves.

## Renewal

Canopy decides when a certificate is due and re-orders on its own; the server's part is to keep collecting.
A chain already in hand stays valid while a renewal is under way, so a renewal in flight is not a failure and a renewed chain replaces the old one without the served certificate lapsing.

## Revocation and key replacement

A certificate Canopy reports as revoked stops being served, and a new one is requested.

A certificate Canopy reports as requiring its key to be replaced gets a new key pair before the next request, rather than a further request against the same key.

## When the grant is absent

A server that may not obtain certificates stops requesting them.

It holds no record of having been entitled before, so a grant that was withdrawn and a grant that was never held are indistinguishable in what the server keeps and in what it reports.
Withdrawal is a containment action taken during an incident, and a host under suspicion is not where that is reasoned about.

Chains already collected continue to be served.
A withdrawn grant is not a revocation; revocation is reported separately and is what takes a chain out of service.

## The collection healthcheck

The `canopy_certificates` check grades the collection.
It is one of the doctor's healthchecks; see [DOC](../tamanu/doctor.md) for the framework it runs in and [CHK](../tamanu/healthchecks.md) for how its outcome is reported.

The check fails when a name is wanted and no chain has been collected for it, and when a collected chain is nearer expiry than renewal should have allowed.
It reports the reason Canopy gave for a name whose order is failing.

The check skips when the server holds no TLS grant, and while Canopy reports the server paused, naming the unmet precondition.
The reason a skip carries states only that the server is not obtaining certificates.
A skip closes an issue the check had already opened, so a grant withdrawn during an incident quietens this check rather than adding to the incident.
A pause is not escalated however long it lasts, Canopy being where it was set.

The `caddy_certs` check grades a chain the daemon serves alongside the certificates Caddy manages itself, so a name being served by Caddy's own issuance because the daemon declines it is visible rather than indistinguishable from one Canopy is serving.

## Commands

`bestool canopy certs` reaches the running daemon over its HTTP interface.
It reports the certificates Canopy holds for this server, requests a name, and runs a collection without waiting for the schedule.

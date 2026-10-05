---
id: TLS
---

# Canopy-issued TLS certificates

A server obtains its TLS certificates from Canopy rather than from a certificate authority it reaches itself.
Canopy holds the account with the authority and proves control of the DNS name through DNS, so the server needs no DNS credential of its own.

The private key never leaves the machine: Canopy signs a certificate signing request and never sees the key behind it.

The alertd daemon runs the collection on a schedule and serves the collected chain to Caddy; see [TLSD](certificate-delivery.md) for how Caddy is served.
Which DNS names a server may certify follows from its entitlement, see [NAM](names.md).
Whether the collection is working is graded by its own healthcheck, see [CHK-CCO](certificate-collection-check.md).
The server authenticates to Canopy with the device identity in its registration, see [CHK-REG](registration.md).

## Which DNS names are certified

A DNS name is certified when it is a site address Caddy is configured to serve, on a site that names the daemon's certificate endpoint as a certificate source, and it belongs to an application on the host that could certify it: within that application's domains, with the TLS grant held and no pause in force ([NAM](names.md#which-application-a-dns-name-belongs-to)).
All of these are read from Caddy's live admin configuration and Canopy's answer.
Caddy's configuration says which DNS names the host answers on and which of those Caddy will ask the daemon for, and the entitlement says which of those Canopy will act on and for which application; a DNS name failing any of the tests is left to Caddy's own issuance.
A site that does not name the daemon's endpoint is never served from Canopy ([TLSD](certificate-delivery.md#caddy-configuration)), so a pass orders nothing for it of its own accord, and on a host where no site names the endpoint a pass orders nothing at all.
The daemon orders for the DNS names meeting every test ahead of any client arriving, because a certificate is obtained before it is needed rather than while a client waits.

A handshake for a configured DNS name the daemon holds no chain for records that DNS name so that an order follows, which covers a DNS name added to Caddy between reads of its configuration.
A DNS name recorded this way is subject to the same tests as one read from the configuration, so a handshake cannot conjure an order for a DNS name outside the server's reach.
Caddy passes the client's own server name through for a site configured on a wildcard, so the DNS name reaching the daemon this way is remote input: the number of DNS names a handshake may leave waiting is bounded, and past the bound a new DNS name is dropped rather than an older one evicted, so a stream of invented DNS names cannot push out the DNS name a real client asked for.
What a pass acts on still comes from Caddy's configuration; a DNS name recorded during a handshake only anticipates the next read of it.
A pass that cannot read Caddy's configuration orders only the DNS names requested by command, and leaves what it knows about every other DNS name as it was, rather than reading Caddy as serving nothing.

A server standing down records nothing to act on, so the DNS names handshakes leave behind while it stands down do not bring a pass forward: it waits for the ordinary interval rather than asking again for an answer that cannot change until the grant returns or the pause lifts.

A DNS name may also be requested explicitly through a command, for pre-provisioning.
A DNS name requested this way is ordered whether or not Caddy serves it yet or its site names the daemon's endpoint, so its chain is ready before the site or the hook is added.
Passes keep collecting it until its first chain arrives, and after that while Caddy serves it.
A DNS name whose application is paused or loses the TLS grant after it was requested is kept and ordered once that is lifted, and until then it does not bring a pass forward.
The number of DNS names requested this way is bounded, and a request past the bound is refused rather than an earlier one forgotten.

## Keys

Each DNS name has its own key pair, generated on the machine, and its own certificate signing request.
A key covering one DNS name means a key Canopy condemns costs only that DNS name a replacement.

Keys are ECDSA over the P-256 curve.

A request carries the signing request as base64-encoded DER and names exactly one DNS name.
Canopy refuses a request whose signing request carries any other name rather than trimming it.
A request carries the type of the application the DNS name belongs to ([NAM](names.md#which-application-a-dns-name-belongs-to)), or for an explicit request, the type the command names.

Keys are held in a machine-bound encrypted store alongside the device identity, keyed by a passphrase derived from the host's machine id, so no private key is at rest in plaintext and the store cannot be read on a different machine.
One store holds every DNS name's key.
The store is readable only by its owner, because the passphrase that unlocks it is derived from the machine id and the machine id is readable by anyone on the host: read access to the file is read access to every key in it.
Collected chains are held beside it in the clear, a chain being public, so a collection that lands rewrites a plain file rather than the encrypted store.
The chains are readable by the group owning the configuration directory, so an unprivileged check grades them; the key store is not.

Keys outlive a daemon restart, so a restart collects an order already placed rather than placing a new one.

## Requesting and collecting

A request and a collection are the same call to Canopy, and it is safe to repeat.
Proving control of a DNS name through DNS takes longer than a client waits, so the first call records the order and answers that it is pending, and a later call collects the chain.

Canopy answers from a certificate it already holds for the same DNS name and key rather than ordering again, so a server that has lost its local copy of a chain costs the authority nothing.

Canopy's answer carries the state of the order, the chain once there is one, when it expires, whether it can be served, whether it has been revoked, whether the key must be replaced, and the reason the last attempt failed while Canopy is still retrying.
A reported error is surfaced rather than retried into.

The entitlement answer lists the certificates Canopy holds, with the DNS name and key each covers and when it expires, but not the chains themselves.
It is therefore what the server reconciles against — which DNS names Canopy already has a certificate for, and whether that certificate covers a key the server still holds — while the chain itself is only ever collected per DNS name.

The daemon collects on a repeating schedule.
A DNS name whose order is pending is retried sooner than the steady-state schedule until the order resolves.

## Renewal

Canopy decides when a certificate is due and re-orders on its own; the server's part is to keep collecting.
A chain already in hand stays valid while a renewal is under way, so a renewal in flight is not a failure and a renewed chain replaces the old one without the served certificate lapsing.

How long a chain lives is Canopy's to choose and is not known before one arrives, the expiry coming back with the chain.
So the collection schedule suits the shortest lifetime Canopy might issue under, and every judgement the server makes about a chain running out is a fraction of that chain's own lifetime rather than a fixed duration.

## Revocation and key replacement

A certificate Canopy reports as revoked stops being served immediately, and a replacement is requested.

Revoking a certificate pauses the server in Canopy, so the replacement request is refused until an operator lifts the pause.
The server therefore stops serving the revoked chain at once and keeps asking for its replacement under the ordinary schedule, rather than treating the refusal as a fault: revocation is an operator acting on this host, and the pause is that operator deciding when it may have a certificate again.
Other DNS names the server holds chains for are unaffected and continue to be served.

A certificate Canopy reports as requiring its key to be replaced gets a new key pair before the next request, rather than a further request against the same key.
A condemned key is never certified again, for any DNS name, so replacing it is the only way forward and the server does not wait for an operator to act on the key itself.

## Undeclared and denied DNS names

A request Canopy refuses as undeclared is waiting on an operator to declare the DNS name in Canopy, and is not a fault on this host ([NAM](names.md#how-canopy-resolves-a-request)).
The daemon keeps asking about it on the steady schedule rather than sooner, and a handshake asking for it does not bring a pass forward.
It keeps asking rather than going quiet, because Canopy shows an operator an undeclared request only while the machine keeps making it, and drops one the machine has not asked about for a day.
The request that follows a declaration is accepted, and collection carries on from there as for any DNS name.

A request Canopy refuses as denied is an operator's decision that no application on this machine should be certified for the DNS name.
The DNS name is left to Caddy's own issuance, as any DNS name Canopy will not certify is.
The daemon keeps asking about it on the steady schedule rather than sooner, which is how a lifted denial is noticed, and a handshake asking for it does not bring a pass forward.
A chain already collected for a denied DNS name continues to be served until it expires, since a denial is not a revocation.

Requesting a DNS name by command ([Commands](#commands)) asks Canopy at once, whatever Canopy last refused it as, so an operator who has just declared a DNS name or lifted its denial need not wait for the steady schedule.

The daemon keeps, for each DNS name, how Canopy last refused it, as undeclared, as denied, or otherwise, with the reason Canopy gave, until a later answer replaces it or the daemon stops asking about the DNS name.
A failure that is not an answer about the DNS name replaces nothing: Canopy being unreachable or failing, asking the daemon to slow down, or answering about the application or the machine rather than the DNS name, such as a pause, a missing grant, or not accepting the machine's identity.
The record survives a daemon restart, so a DNS name waiting on an operator is not mistaken for a failing one before the daemon has asked again.
That record is what the certificate healthcheck reads to tell a DNS name waiting on an operator from one whose collection is failing, and why a failing one is failing ([CHK-CCO](certificate-collection-check.md)).
It is kept beside the collected chains, written only by the daemon and readable without privilege, as reporting what this server holds is.

## When the grant is absent or the server is paused

A server that may not obtain certificates stops requesting them, and so does one Canopy reports as paused.

Chains already collected continue to be served in both cases.
Neither a withdrawn grant nor a pause is a revocation, and revocation is what takes a chain out of service; see [TLSD](certificate-delivery.md) for what the daemon serves.
So withdrawing a grant from a host under suspicion stops it obtaining anything new without also dropping every DNS name it currently answers on.

A server holds no record of having been entitled before, so a grant that was withdrawn and a grant that was never held are indistinguishable in what it keeps and in what it reports.
Withdrawal is a containment action taken during an incident, and a host under suspicion is not where that is reasoned about.

A pause is Canopy's to lift and no length of pause is escalated from here, so a paused server waits rather than retrying against the refusal.

## Commands

`bestool canopy certs` reaches the running daemon over its HTTP interface.
It reports the certificates Canopy holds for this server, requests a DNS name, and runs a collection without waiting for the schedule.

Its report shows, for each DNS name whose order is in flight or failing, the application type the request carried, and the state of the order.
A DNS name Canopy refused shows the refusal as its state, undeclared, denied, or otherwise, with the reason Canopy gave.

Requesting a DNS name requires the application it is for, named by its type, because a DNS name requested ahead of its site cannot be attributed from Caddy's configuration.
Requesting a DNS name and running a collection spend orders at the authority, so they are refused unless run by the superuser; reporting what this server holds needs no privilege.
Requesting a DNS name outside the domains the named application's group controls is refused rather than reported as taken, because a pass would drop it.
Requesting a DNS name for an application that is paused or without the TLS grant is refused with that reason, rather than reported as taken while nothing is ordered.

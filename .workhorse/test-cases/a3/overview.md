# Ask Canopy for names and certificates — test cases

Coverage owed by this card. An unticked case is a scenario not yet covered, not one decided against.

## End to end

- [ ] A server with the TLS grant, on a name inside its group's domains, obtains a certificate from Canopy and Caddy serves it (verifies spec: TLS).
- [ ] A server issuing this way makes no DNS-zone access of its own (verifies spec: TLS).

## Precedence and fallback

- [ ] With a chain held for a name, Caddy serves Canopy's and places no order of its own (verifies spec: TLSD#precedence-over-caddys-own-issuance).
- [ ] With no chain held, the endpoint declines and Caddy issues for the name itself (verifies spec: TLSD#precedence-over-caddys-own-issuance).
- [ ] A name served from Canopy and a name that fell back to Caddy coexist on one host (verifies spec: TLSD).
- [ ] The endpoint declines rather than errors for each case it cannot serve: name out of domain, nothing collected yet, chain revoked, no TLS grant, server paused (verifies spec: TLSD#declining-and-failing).
- [ ] A name no site is configured for never reaches the endpoint (verifies spec: TLSD#caddy-configuration).

## Keys and requests

- [ ] The private key is generated on the machine and appears in no request body; the request carries a signing request only (verifies spec: TLS#keys).
- [ ] A signing request naming more than the one name is refused rather than trimmed (verifies spec: TLS#keys).
- [ ] Each name has its own key, so replacing one name's key leaves the other names' certificates untouched (verifies spec: TLS#keys).
- [ ] Keys survive a daemon restart: a restart collects an order already placed rather than placing a new one (verifies spec: TLS#keys).
- [ ] The key store cannot be read on a different machine (verifies spec: TLS#keys).

## Ordering and collection

- [ ] A first request answers pending and a later collection lands the chain (verifies spec: TLS#requesting-and-collecting).
- [ ] A name whose order is pending is retried sooner than the steady-state schedule, and stops being retried once it resolves (verifies spec: TLS#requesting-and-collecting).
- [ ] A repeated request for a name and key Canopy already holds is answered from what it holds rather than ordering again (verifies spec: TLS#requesting-and-collecting).
- [ ] A name appearing in Caddy's active subjects is ordered before any client arrives (verifies spec: TLS#which-names-are-certified).
- [ ] A handshake for a configured in-domain name with nothing held records the name and an order follows (verifies spec: TLS#which-names-are-certified).
- [ ] A renewal replaces the chain without the served certificate lapsing, and without a Caddy reload (verifies spec: TLS#renewal).

## Entitlement

- [ ] A name outside the group's domains is not acted on (verifies spec: NAM#entitlement).
- [ ] A machine answered with an applications list acts on the union of their domains and grants (verifies spec: NAM#machines-hosting-several-applications).
- [ ] A server Canopy reports as paused makes no requests until the pause lifts (verifies spec: NAM#entitlement).
- [ ] A server with no grants receives an empty answer rather than an error (verifies spec: NAM#entitlement).

## Revocation, key replacement, withdrawal

- [ ] A revoked certificate stops being served and a new one is requested (verifies spec: TLS#revocation-and-key-replacement).
- [ ] A certificate requiring its key replaced gets a new key pair before the next request (verifies spec: TLS#revocation-and-key-replacement).
- [ ] A server whose TLS grant is withdrawn stops requesting, keeps serving the chains it holds, and reports nothing (verifies spec: TLS#when-the-grant-is-absent).
- [ ] A server that never held a grant and one that has lost it keep and report the same thing (verifies spec: TLS#when-the-grant-is-absent).
- [ ] A reported error from Canopy is surfaced rather than retried into (verifies spec: TLS#requesting-and-collecting).

## The delivery endpoint

- [ ] A held chain is served while Canopy is unreachable, the answer coming from memory (verifies spec: TLSD#the-certificate-endpoint).
- [ ] A stalled Canopy call does not stall a handshake (verifies spec: TLSD#the-certificate-endpoint).
- [ ] A caller that is not the superuser or the configured permitted user is refused, and the refusal is a failure rather than a decline (verifies spec: TLSD#who-may-fetch-a-certificate).

## DNS

- [ ] Registering a name publishes an A record per IPv4 address and an AAAA record per IPv6 address (verifies spec: NAM#registering-addresses-for-a-name).
- [ ] Re-registering a name replaces the addresses registered before (verifies spec: NAM#registering-addresses-for-a-name).
- [ ] Registering a name with no addresses withdraws it (verifies spec: NAM#registering-addresses-for-a-name).
- [ ] A registration is answered before the zone has caught up, reporting what has been published so far (verifies spec: NAM#registering-addresses-for-a-name).

## Commands

- [ ] `bestool canopy certs` reports what Canopy holds, requests a name, and runs a collection off-schedule (verifies spec: TLS#commands).
- [ ] `bestool canopy dns` registers, withdraws, and reports registrations with their published state (verifies spec: NAM#commands).
- [ ] Both reach a running daemon, and report usefully when none is running.

## Healthchecks

- [ ] The check fails when a name is wanted and no chain has been collected for it (verifies spec: TLS#the-collection-healthcheck).
- [ ] The check fails when a collected chain is nearer expiry than renewal should have allowed (verifies spec: TLS#the-collection-healthcheck).
- [ ] The check skips, naming the precondition, with no TLS grant and while paused (verifies spec: TLS#the-collection-healthcheck).
- [ ] A skip closes an issue the check had already opened (verifies spec: TLS#the-collection-healthcheck).
- [ ] A host serving every name from Caddy's own issuance is distinguishable from one Canopy is serving (verifies spec: TLS#the-collection-healthcheck).

## Operational

- [ ] Deploying the Caddyfile shape on a host that still holds its DNS credential leaves the host working throughout.
- [ ] Withdrawing the DNS credential afterwards leaves Canopy-served names working.

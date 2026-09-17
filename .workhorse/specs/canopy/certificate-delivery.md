---
id: TLSD
---

# Serving Canopy-issued certificates to Caddy

Caddy obtains a Canopy-issued chain from the alertd daemon during the TLS handshake, rather than from a file it has been handed.
A chain that is collected or renewed is served without a Caddy reload.

Canopy takes precedence over Caddy's own issuance rather than replacing it: a name Canopy serves is served from Canopy, and a name it does not is issued by Caddy as it would be otherwise.
The certificates themselves are described in [TLS](certificates.md).

## The certificate endpoint

The daemon serves a certificate endpoint on its local HTTP interface.
Caddy requests it during a handshake, passing the name from the client's server name indication along with the signature schemes and cipher suites the client offered and the local address it connected to.

When the daemon holds a chain for the name that can be served, it answers with that chain and its private key together as PEM.
A chain can be served when Canopy reports it usable: neither revoked nor past its expiry.
Otherwise it declines.

Every answer comes from state the daemon already holds in memory.
The endpoint reaches no network, opens no key store and reads no file: the handshake is blocked until it answers, and Caddy applies no timeout to the request.

## Precedence over Caddy's own issuance

Caddy consults the daemon before any certificate it holds or manages itself.

A name the daemon answers for is served from Canopy, and Caddy obtains no certificate of its own for it.
A name the daemon declines is issued by Caddy exactly as it would be were the daemon not configured at all.

So a host serves whether or not Canopy is answering, and the DNS credential Caddy uses for its own issuance is withdrawn once Canopy is answering reliably, rather than being a precondition for configuring this at all.

## Declining and failing

A decline and a failure are distinct, and the endpoint declines wherever it can.

A decline returns no content, and hands the handshake back to Caddy to issue for the name itself.
The endpoint declines for a name outside the domains the server's group controls, a name with no chain collected yet, and a chain that is no longer usable because it has been revoked or has expired.

What the endpoint serves turns on the chain in hand rather than on what the server may currently ask for.
A withdrawn TLS grant and a pause both stop new requests ([TLS](certificates.md)) and neither takes a collected chain out of service, so a grant withdrawn under an incident does not also drop every name the host serves.
Revocation is what takes a chain out of service, and it is reported separately from both.

A failure ends the handshake.
Caddy reads it as the daemon having been unable to serve a certificate it was responsible for, and does not fall back to its own issuance, so a failure takes down a name Caddy would otherwise have covered.
The endpoint fails only when it cannot answer at all.

## Caddy configuration

A site served this way names the daemon's certificate endpoint as a certificate source for the addresses that site serves, and keeps its own issuer configured.

Caddy's certificate management stays enabled, because its own issuance is the fallback a decline reaches.

A name no site is configured for never reaches the endpoint, and no certificate is issued for it by either party.

## Who may fetch a certificate

The endpoint hands out a private key, so it identifies its caller rather than accepting any connection that reaches it.

The daemon resolves the connecting process and the user it runs as, and serves the superuser and one further user named in the daemon's configuration, which is the user Caddy runs as.

A caller that is not permitted is refused, and that refusal is a failure rather than a decline: it is a misconfiguration to correct, not a name for Caddy to begin issuing for itself.

---
id: NAM
---

# Names a server may use

Canopy controls the domains a group holds, and grants a server the right to act on names within them.
A server asks Canopy what it may do rather than holding that answer locally, because a grant can be withdrawn at any time.

The grants for DNS and for TLS are separate: a server may be entitled to publish addresses for a name, to obtain certificates for it, to both, or to neither.
Certificates are described in [TLS](certificates.md).

## Entitlement

Canopy's answer carries the domains the server's group controls, whether the server may manage its own DNS records, whether it may obtain its own TLS certificates, whether Canopy is currently paused for this server, the names the server has registered addresses for, and the certificates Canopy holds for it.

A server with no grants, or whose group controls no domain, receives an empty answer rather than an error.
Asking what one may do is not itself a privileged act.

A name is within the server's reach when it sits at or beneath one of the domains the group controls.
A name outside them is not acted on.

While Canopy reports the server paused it is making no changes on the server's behalf and refuses requests, so the server makes none until the pause lifts.

## Machines hosting several applications

Canopy describes a machine hosting a single application in the top-level fields of its answer, and a machine hosting several as a list of applications, each with its own domains, grants and paused state.

A server acts on the union of what it is told: the top-level fields when those carry the answer, and every application's domains and grants together when they do not.
A name is actionable when any application on the machine could act on it.

## Registering addresses for a name

A server publishes the addresses a name resolves to by registering them with Canopy, which publishes an A record for each IPv4 address and an AAAA record for each IPv6 address.
A machine needs no access to the DNS zone of its own.

A registration names one name and the addresses it resolves to, and replaces whatever addresses were registered for that name before.
The name must sit within a domain the group controls and the server must hold the DNS grant.

Canopy publishes what it is told, and does not verify that an address belongs to the server; the grant is the trust boundary.

Publishing happens in the background, so a registration is answered with the addresses Canopy will publish and those it has published so far, rather than waiting for the zone to catch up.

Registering a name with no addresses withdraws it: the records are taken down and the name is freed.

## Commands

`bestool canopy dns` registers a name's addresses, withdraws a name, and reports the names Canopy holds registrations for on this server, with the addresses it has published and whether the zone has caught up.

Registration is driven by these commands.
Publishing addresses directs traffic at a host, so it follows an operator's instruction rather than a periodic reconciliation.

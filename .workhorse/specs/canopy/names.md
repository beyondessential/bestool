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

Grants, domains and the paused state belong to an application rather than to the machine, and a machine may host several.
Canopy describes a machine hosting exactly one application in the top-level fields of its answer, and a machine hosting several, or none, by leaving those fields empty and listing the applications, each with its own domains, grants and paused state.

A server acts on the union of what it is told: the top-level fields when those carry the answer, and every application's domains and grants together when they do not.
A name is treated as actionable when any application on the machine could act on it.
Nothing on this side knows which application a Caddy site belongs to, so the union is the best the server can do, and it errs towards asking.

Canopy resolves which application a request concerns from the name it asks about, not from the identity presented, because an identity belongs to the machine.
So a request the union produced can still be refused — for a name no application on the machine holds, or held by an application that lacks the grant or is paused — and that refusal is authoritative.
A refusal of this kind is reported as it is given rather than retried against a different application, there being no other application to ask as.

The union governs only what the server asks for.
What it holds is still attributed per application, because Canopy's answer says which application declares each name, and that is what the certificate healthcheck reports against ([CHK-CCO](certificate-collection-check.md)).
So asking as the machine and reporting per application sit together rather than in tension.

## Registering addresses for a name

A server publishes the addresses a name resolves to by registering them with Canopy, which publishes an A record for each IPv4 address and an AAAA record for each IPv6 address.
A machine needs no access to the DNS zone of its own.

A registration names one name and the addresses it resolves to, and replaces whatever addresses were registered for that name before.
The name must sit within a domain the group controls and the server must hold the DNS grant.

A name belongs to one application across the whole fleet, so registering a name another server already holds is refused, and the refusal is reported rather than worked around.
Two hosts cannot both publish addresses for one name, which is what stops a name being pulled between them.

Canopy publishes what it is told, and does not verify that an address belongs to the server; the grant is the trust boundary.

Publishing happens in the background, so a registration is answered with the addresses Canopy will publish and those it has published so far, rather than waiting for the zone to catch up.

Registering a name with no addresses withdraws it: the records are taken down and the name is freed.

## Commands

`bestool canopy dns` registers a name's addresses, withdraws a name, and reports the names Canopy holds registrations for on this server, with the addresses it has published, whether the zone has caught up, and why the last publish failed if one did.

Registration is driven by these commands.
Publishing addresses directs traffic at a host, so it follows an operator's instruction rather than a periodic reconciliation.

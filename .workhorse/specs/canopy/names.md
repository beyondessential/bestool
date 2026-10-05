---
id: NAM
---

# DNS names a server may use

Canopy controls the domains a group holds, and grants a server the right to act on DNS names within them.
A server asks Canopy what it may do rather than holding that answer locally, because a grant can be withdrawn at any time.

The grants for DNS and for TLS are separate: a server may be entitled to publish addresses for a DNS name, to obtain certificates for it, to both, or to neither.
Certificates are described in [TLS](certificates.md).

## Entitlement

Canopy's answer carries the domains the server's group controls, whether the server may manage its own DNS records, whether it may obtain its own TLS certificates, whether Canopy is currently paused for this server, the DNS names the server declares, whether by registering addresses for them or by an operator declaring them, and the certificates Canopy holds for it.

A server with no grants, or whose group controls no domain, receives an empty answer rather than an error.
Asking what one may do is not itself a privileged act.

A DNS name is within the server's reach when it sits at or beneath one of the domains the group controls.
A DNS name outside them is not acted on.

While Canopy reports the server paused it is making no changes on the server's behalf and refuses requests, so the server makes none until the pause lifts.

## Machines hosting several applications

Grants, domains and the paused state belong to an application rather than to the machine, and a machine may host several.
Canopy describes a machine hosting exactly one application in the top-level fields of its answer, and a machine hosting several, or none, by leaving those fields empty and listing the applications, each with its own domains, grants and paused state.

A server acts on the union of what it is told: the top-level fields when those carry the answer, and every application's domains and grants together when they do not.
A DNS name is treated as actionable when any application on the machine could act on it.

### Which application a site belongs to

The server attributes a Caddy site to an application on the host where it can tell, and leaves it unattributed where it cannot.
A site belongs to the host's Tamanu when its address is the host of Tamanu's configured canonical URL, or when every upstream it proxies to is one of that Tamanu's own: a service name its deployment gives it, or the local port it is configured to listen on.
A site belongs to the host's mSupply when every upstream it proxies to is one of mSupply's own: a service name its deployment gives it, or the local port it listens on.
A site whose upstreams belong to more than one application, or to none the server recognises, is unattributed.

A request about a DNS name a site serves carries the type of the application that site belongs to, and a request about an unattributed DNS name carries no type.
That holds for certificate requests and for address registrations alike.

### How Canopy resolves a request

Canopy resolves which application a request concerns from the DNS name it asks about and from the type the request carries, not from the identity presented, because an identity belongs to the machine.
A DNS name an application on the machine already declares resolves to that application; otherwise the type narrows it; otherwise the one application holding the needed grant whose domains cover the DNS name.
A request that resolves declares the DNS name for that application, so later requests follow the declaration.

A request that resolves to no single application is refused as undeclared, and its remedy is an operator declaring the DNS name in Canopy.
The server treats it as waiting on that operator rather than as a fault ([TLS](certificates.md#undeclared-and-denied-dns-names)).

A DNS name an operator has denied to the machine is refused as denied, and the server treats it as a decision against that DNS name rather than as a fault ([TLS](certificates.md#undeclared-and-denied-dns-names)).

A request whose type differs from the application already declaring the DNS name is refused, naming the declaring application's type.
The server sends the type its own attribution gives regardless of the declaration, so a site Caddy routes to one application while Canopy holds it for another is reported as a fault for an operator to correct.

Any other refusal is reported as it is given rather than retried against a different application.

The union governs only what the server asks for.
What it holds is still attributed per application, because Canopy's answer says which application declares each DNS name, and that is what the certificate healthcheck reports against ([CHK-CCO](certificate-collection-check.md)).
So asking as the machine and reporting per application sit together rather than in tension.

## Registering addresses for a DNS name

A server publishes the addresses a DNS name resolves to by registering them with Canopy, which publishes an A record for each IPv4 address and an AAAA record for each IPv6 address.
A machine needs no access to the DNS zone of its own.

A registration names one DNS name and the addresses it resolves to, and replaces whatever addresses were registered for that DNS name before.
It carries the type of the application the DNS name's site belongs to, where the server can tell ([Which application a site belongs to](#which-application-a-site-belongs-to)).
The DNS name must sit within a domain the group controls and the server must hold the DNS grant.

A DNS name belongs to one application across the whole fleet, so registering a DNS name another application already declares is refused, and the refusal is reported with the reason Canopy gave rather than worked around.
Two hosts cannot both publish addresses for one DNS name, which is what stops a DNS name being pulled between them.

Canopy publishes what it is told, and does not verify that an address belongs to the server; the grant is the trust boundary.
An address that is not an IP address is refused, naming the offending value, before Canopy is asked: the server holding the DNS grant is where an address is checked, whatever asked it to register one.

Publishing happens in the background, so a registration is answered with the addresses Canopy will publish and those it has published so far, rather than waiting for the zone to catch up.

Registering a DNS name with no addresses withdraws it: the records are taken down and the DNS name is freed.

## Commands

`bestool canopy dns` registers a DNS name's addresses, withdraws a DNS name, and reports the DNS names Canopy holds registrations for on this server, with the addresses it has published, whether the zone has caught up, and why the last publish failed if one did.

Registration is driven by these commands.
Publishing addresses directs traffic at a host, so it follows an operator's instruction rather than a periodic reconciliation.

Registering and withdrawing change what the world resolves for a production deployment, so they are refused unless run by the superuser, and a refusal says so.
The daemon's interface is reachable by every process on the host, and a command that only reports is not: reading what this server holds needs no privilege.

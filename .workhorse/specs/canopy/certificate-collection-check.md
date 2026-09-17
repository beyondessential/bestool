---
id: CHK-CCO
---

# The Canopy certificate collection check

The `canopy_certificates` check grades whether this server is holding the certificates it should be getting from Canopy.
It is one of the doctor's healthchecks; see [DOC](../tamanu/doctor.md) for the framework it runs in and [CHK](../tamanu/healthchecks.md) for how its outcome is reported.

What it grades is the collection: that a name the server ought to have a chain for has one, and that the chain is not running down.
Obtaining and serving the chains themselves is described in [TLS](certificates.md), and the entitlement the check reads is described in [NAM](names.md).

## Which names it grades

The check grades the names [TLS](certificates.md#which-names-are-certified) says are certified: a site address Caddy serves that the server's entitlement also covers.
A name Caddy serves that the entitlement does not cover is not this check's business, because nothing should be collecting a chain for it.

## Outcomes

The check fails when a name it grades has no chain collected for it.

It fails when a collected chain is nearer expiry than renewal should have allowed.
Canopy re-orders on its own and the server keeps collecting, so a chain that has run down this far means the collection has stopped working rather than that a renewal is merely in flight.
How near is too near is a fraction of that chain's own lifetime, since Canopy chooses the lifetime and a fixed duration would fire far too late for a short-lived chain and far too early for a long-lived one.

A renewal under way is not a failure: the chain in hand stays valid until the new one lands, so a name holding a usable chain passes whatever Canopy is doing behind it.

The check reports the reason Canopy gave for a name whose order is failing, so an operator sees why issuance is stuck rather than only that nothing arrived.

## When it skips

The check skips when the server holds no TLS grant, and while Canopy reports the server paused, naming the unmet precondition.
The reason a skip carries states only that the server is not obtaining certificates.

A skip closes an issue the check had already opened, so a grant withdrawn during an incident quietens this check rather than adding to the incident.
A pause is not escalated however long it lasts, Canopy being where it was set, and Canopy is what reports a pause old enough to have let something lapse.

A revoked certificate pauses the server in Canopy ([TLS](certificates.md#revocation-and-key-replacement)), so a revocation quietens this check by the same route.
That is the intent: the operator who revoked the certificate is acting on the host already, and does not need this check telling them the chain they just revoked is missing.

## Alongside Caddy's own certificates

The `caddy_certs` check ([CHK-CCT](../tamanu/caddy-certs.md)) grades every certificate Caddy serves, including those Caddy issues for itself.
It distinguishes a chain served from Canopy from one Caddy obtained itself, so a host that has quietly fallen back to Caddy's own issuance is visible rather than looking the same as one Canopy is serving.

Without that distinction a host could sit indefinitely on Caddy's own issuance, still holding the DNS credential this work exists to remove, and present as healthy.

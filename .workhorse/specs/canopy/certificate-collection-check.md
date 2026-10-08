---
id: CHK-CCO
---

# The Canopy certificate collection check

The `canopy_certificates` check grades whether this server is holding the certificates it should be getting from Canopy.
It is one of the doctor's healthchecks; see [DOC](../tamanu/doctor.md) for the framework it runs in and [CHK](../tamanu/healthchecks.md) for how its outcome is reported.

What it grades is the collection: that a DNS name the server ought to have a chain for has one, and that the chain is not running down.
Obtaining and serving the chains themselves is described in [TLS](certificates.md), and the entitlement the check reads is described in [NAM](names.md).

## Which subject it reports for

The check reports for an application, not for the machine, and runs once for each Tamanu and each mSupply application on the host ([SUBJ](../tamanu/subjects.md)).

Certificates belong to the application they were issued for.
A grant, a pause, and the domains a DNS name must sit under are each an application's own, and the group that answers for a failing certificate is the application's group: a machine may host two applications belonging to different groups, so a result filed against the machine would reach the wrong people for one of them.

Filing per application also keeps the heal attempts and backoff of [CHK](../tamanu/healthchecks.md#self-healing) separate, so one application's stalled collection does not consume another's allowance.

## Which DNS names it grades

The check grades the DNS names [TLS](certificates.md#which-dns-names-are-certified) says are certified that belong to *its own* application ([NAM](names.md#which-application-a-dns-name-belongs-to)).
A site that does not name the daemon's endpoint can never be served from Canopy, so its DNS names are not graded, and a host where no site names the endpoint has nothing to grade.

An application's entry is matched to the application the check is running for by the application type, which is what Canopy puts on the wire for a reporter to correlate against; on a machine hosting a single application Canopy gives that entry as the answer itself.

The check reads the daemon's record of which DNS names Canopy last refused, how, and with what reason ([TLS](certificates.md#undeclared-and-denied-dns-names)).
It reads that record from where the daemon keeps it rather than asking the running daemon, since the record decides which DNS names go ungraded and an answer over the daemon's local interface could come from any process holding its port, where the record is held to the same trust as the chains the check grades.
Where the record cannot be read, the check grades only the DNS names Canopy's answer says this application declares, since a DNS name it does not declare may be waiting on an operator and nothing here can tell, and it says in its detail that the record could not be read.

The entitlement, Caddy's configuration, and the collected chains are each one answer for the machine, so a sweep takes each once and every application's run of the check reads the same one.
Two checks in a sweep cannot disagree about what the host serves or holds, and a machine carrying several applications costs one reading rather than one per application.

## Outcomes

The check fails when a DNS name it grades has no chain collected for it.

It fails when a collected chain is nearer expiry than renewal should have allowed.
Canopy re-orders on its own and the server keeps collecting, so a chain that has run down this far means the collection has stopped working rather than that a renewal is merely in flight.
How near is too near is a fraction of that chain's own lifetime, since Canopy chooses the lifetime and a fixed duration would fire far too late for a short-lived chain and far too early for a long-lived one.

A renewal under way is not a failure: the chain in hand stays valid until the new one lands, so a DNS name holding a usable chain passes whatever Canopy is doing behind it.

The check reports the reason Canopy gave for a DNS name whose order is failing, whether Canopy gave it on a certificate it holds or in refusing the daemon's last request, so an operator sees why issuance is stuck rather than only that nothing arrived.
A request refused as a type mismatch is such a failure, and the reason names the types Canopy gave ([NAM](names.md#how-canopy-resolves-a-request)).

A DNS name Canopy refused as undeclared or as denied is not graded, whether or not a chain is held for it.
Canopy shows an operator an undeclared request itself, and a denial is an operator's decision against the DNS name, so neither is this host's to report.
Each is reported as a skipped instance whose detail says whether it was undeclared or denied, with Canopy's reason, and changes neither the outcome nor the summary.

## Reporting

The check reports one instance per DNS name it lists, keyed by the DNS name, as described in [CHK](../tamanu/healthchecks.md), so one name failing to collect is graded and silenced without quieting the others.
A graded name is passed or failed.
Its detail carries whether a chain is collected, the days remaining, whether Canopy holds a certificate for it, and the reason its order is failing, if any.
Where the refusal record could not be read, the check's detail says so and that only declared names were graded.

## When it skips

The check skips when its application holds no TLS grant, and while Canopy reports that application paused, naming the unmet precondition.
The reason a skip carries states only that the application is not obtaining certificates.

A grant or a pause is an application's own, so one application skipping leaves the others on the machine graded as they were.

A skip closes an issue the check had already opened, so a grant withdrawn during an incident quietens this check rather than adding to the incident.
A pause is not escalated however long it lasts, Canopy being where it was set, and Canopy is what reports a pause old enough to have let something lapse.

A revoked certificate pauses its application in Canopy ([TLS](certificates.md#revocation-and-key-replacement)), so a revocation quietens this check for that application by the same route, and leaves the others on the machine reporting.
That is the intent: the operator who revoked the certificate is acting on the host already, and does not need this check telling them the chain they just revoked is missing.

## Alongside Caddy's own certificates

The `caddy_certs` check ([CHK-CCT](../tamanu/caddy-certs.md)) grades every certificate Caddy serves for a DNS name belonging to an application, including those Caddy issues for itself.
It distinguishes a chain served from Canopy from one Caddy obtained itself, so a host that has quietly fallen back to Caddy's own issuance is visible rather than looking the same as one Canopy is serving.

Without that distinction a host could sit indefinitely on Caddy's own issuance, still holding the DNS credential this work exists to remove, and present as healthy.

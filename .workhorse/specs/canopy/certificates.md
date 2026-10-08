---
id: TLS
---

# TLS certificates

A server obtains its TLS certificates from Canopy rather than from a certificate authority it reaches itself.
Canopy holds the account with the authority and proves control of the DNS name through DNS, so the server needs no DNS credential of its own.

The private key never leaves the machine: Canopy signs a certificate signing request and never sees the key behind it.

The alertd daemon runs the collection on a schedule and serves the collected chain to Caddy; see [Serving certificates to Caddy](#serving-certificates-to-caddy).
Which DNS names a server may certify follows from its entitlement, see [NAM](names.md).
Whether the collection is working, and what Caddy is serving, is graded by healthchecks; see [Healthchecks](#healthchecks).
The server authenticates to Canopy with the device identity in its registration, see [CHK-REG](registration.md).

## Which DNS names are certified

A DNS name is certified when it is a site address Caddy is configured to serve, on a site that names the daemon's certificate endpoint as a certificate source, and it belongs to an application on the host that could certify it: within that application's domains, with the TLS grant held and no pause in force ([NAM](names.md#which-application-a-dns-name-belongs-to)).
All of these are read from Caddy's live admin configuration and Canopy's answer.
Caddy's configuration says which DNS names the host answers on and which of those Caddy will ask the daemon for, and the entitlement says which of those Canopy will act on and for which application; a DNS name failing any of the tests is left to Caddy's own issuance.
A site that does not name the daemon's endpoint is never served from Canopy ([Caddy configuration](#caddy-configuration)), so a pass orders nothing for it of its own accord, and on a host where no site names the endpoint a pass orders nothing at all.
The daemon orders for the DNS names meeting every test ahead of any client arriving, because a certificate is obtained before it is needed rather than while a client waits.

A handshake for a configured DNS name the daemon holds no chain for records that DNS name so that an order follows, which covers a DNS name added to Caddy between reads of its configuration.
A DNS name recorded this way is subject to the same tests as one read from the configuration, so a handshake cannot conjure an order for a DNS name outside the server's reach.
Caddy passes the client's own server name through for a site configured on a wildcard, so the DNS name reaching the daemon this way is remote input: the number of DNS names a handshake may leave waiting is bounded, and past the bound a new DNS name is dropped rather than an older one evicted, so a stream of invented DNS names cannot push out the DNS name a real client asked for.
What a pass acts on still comes from Caddy's configuration; a DNS name recorded during a handshake only anticipates the next read of it.
A pass that cannot read Caddy's configuration, or cannot tell which applications are on the host, orders only the DNS names requested by command, and leaves what it knows about every other DNS name as it was, rather than reading the host as serving nothing.
The next pass waits for the retry interval rather than following on the next tick.
The applications on the host are looked for again on each steady pass, and where looking fails the ones last found stand.

A server standing down records nothing to act on, so the DNS names handshakes leave behind while it stands down do not bring a pass forward: it waits for the ordinary interval rather than asking again for an answer that cannot change until the grant returns or the pause lifts.

A DNS name may also be requested explicitly through a command, for pre-provisioning.
A DNS name requested this way is ordered whether or not Caddy serves it yet or its site names the daemon's endpoint, so its chain is ready before the site or the hook is added.
Passes keep collecting it until its first chain arrives, and after that while Caddy serves it.
Until its first chain arrives it is asked about as promptly as a DNS name a handshake recorded, and from then on it is renewed on the same schedule as any other DNS name.
The request survives a daemon restart.
A request Canopy refuses as made, with a type mismatch or a DNS name outside the named application's domains, is dropped, and the command that made it reports the refusal.
A request Canopy cannot act on for a reason on its own side, such as no zone it manages covering the DNS name, is kept for when that is put right.
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

A DNS name Canopy refused, however it refused it, is asked about again on the steady schedule rather than sooner, since asking sooner earns the same answer, and a handshake asking for it does not bring a pass forward.

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
That record is what the certificate healthcheck reads to tell a DNS name waiting on an operator from one whose collection is failing, and why a failing one is failing ([the collection check](#the-collection-check)).
It is kept beside the collected chains, under the same ownership and permissions, and is readable without privilege, as reporting what this server holds is.
Whatever may change the collected chains may change the record, and nothing else can.

## When the grant is absent or the server is paused

A server that may not obtain certificates stops requesting them, and so does one Canopy reports as paused.

Chains already collected continue to be served in both cases.
Neither a withdrawn grant nor a pause is a revocation, and revocation is what takes a chain out of service; see [Serving certificates to Caddy](#serving-certificates-to-caddy) for what the daemon serves.
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
Requesting a DNS name for an application type the machine does not host is refused, naming the types it does host; a machine Canopy answers for as a single application is taken to host whatever type is named, as Canopy takes it.

## Serving certificates to Caddy

Caddy obtains a Canopy-issued chain from the alertd daemon during the TLS handshake, rather than from a file it has been handed.
A chain that is collected or renewed is served without a Caddy reload.

Canopy takes precedence over Caddy's own issuance rather than replacing it: a name Canopy serves is served from Canopy, and a name it does not is issued by Caddy as it would be otherwise.

### The certificate endpoint

The daemon serves a certificate endpoint on its local HTTP interface.
Caddy requests it during a handshake, passing the name from the client's server name indication along with the signature schemes and cipher suites the client offered and the local address it connected to.

When the daemon holds a chain for the name that can be served, it answers with that chain and its private key together as PEM.
A chain can be served when Canopy reports it usable: neither revoked nor past its expiry.
Otherwise it declines.

Every answer comes from state the daemon already holds in memory.
The endpoint reaches no network, opens no key store and reads no file: the handshake is blocked until it answers, and Caddy applies no timeout to the request.

### Precedence over Caddy's own issuance

Caddy consults the daemon before any certificate it holds or manages itself.

A name the daemon answers for is served from Canopy, and Caddy obtains no certificate of its own for it.
A name the daemon declines is issued by Caddy exactly as it would be were the daemon not configured at all.

So a host serves whether or not Canopy is answering, and the DNS credential Caddy uses for its own issuance is withdrawn once Canopy is answering reliably, rather than being a precondition for configuring this at all.

### Declining and failing

A decline and a failure are distinct, and the endpoint declines wherever it can.

A decline returns no content, and hands the handshake back to Caddy to issue for the name itself.
The endpoint declines for a name outside the domains the server's group controls, a name with no chain collected yet, and a chain that is no longer usable because it has been revoked or has expired.

What the endpoint serves turns on the chain in hand rather than on what the server may currently ask for.
A withdrawn TLS grant and a pause both stop new requests ([When the grant is absent or the server is paused](#when-the-grant-is-absent-or-the-server-is-paused)) and neither takes a collected chain out of service, so a grant withdrawn under an incident does not also drop every name the host serves.
Revocation is what takes a chain out of service, and it is reported separately from both.

A failure ends the handshake.
Caddy reads it as the daemon having been unable to serve a certificate it was responsible for, and does not fall back to its own issuance, so a failure takes down a name Caddy would otherwise have covered.
The endpoint fails only when it cannot answer at all.

### Caddy configuration

A site served this way names the daemon's certificate endpoint as a certificate source for the addresses that site serves, and keeps its own issuer configured.

Caddy's certificate management stays enabled, because its own issuance is the fallback a decline reaches.

A name no site is configured for never reaches the endpoint, and no certificate is issued for it by either party.

The endpoint is `/certificate` on the daemon's local HTTP interface, and a per-site block is what names it:

```caddyfile
app.example.com {
	tls {
		get_certificate http http://127.0.0.1:8271/certificate
	}
	# everything else about the site, including its issuer, unchanged
}
```

Per-site rather than a catch-all automation policy: a catch-all lets an unconfigured server name indication reach the endpoint, and a decline then falls through to on-demand issuance, which Caddy retries for weeks per name.
The cost of per-site blocks is not learning a name Caddy is not configured for, which is a name this server would not certify anyway.

bestool does not write or patch the Caddyfile; the shape is an interface the deployment applies.

### Who may fetch a certificate

The endpoint hands out a private key, so it identifies its caller rather than accepting any connection that reaches it.

The daemon resolves the user that owns the connection, and serves the superuser and one further user named in the daemon's configuration, which is the user Caddy runs as.
Which user owns a connection is what the kernel already records against the socket, so resolving it needs no privilege to read another user's processes.

A caller that is not permitted is refused, and that refusal is a failure rather than a decline: it is a misconfiguration to correct, not a name for Caddy to begin issuing for itself.

## Healthchecks

Two of the doctor's healthchecks grade certificates: the collection check, `canopy_certificates`, and the Caddy check, `caddy_certs`.
See [DOC](../tamanu/doctor.md) for the framework they run in and [CHK](../tamanu/healthchecks.md) for how their outcomes are reported.

Both report for an application rather than for the machine, and run once for each Tamanu and each mSupply application on the host ([SUBJ](../tamanu/subjects.md)), grading the DNS names that belong to that application ([NAM](names.md#which-application-a-dns-name-belongs-to)).
A certificate belongs to the application it was issued for.
A grant, a pause, and the domains a DNS name must sit under are each an application's own, and the group that answers for a failing or expiring certificate is the application's group: a machine may host two applications belonging to different groups, so a result filed against the machine would reach the wrong people for one of them.
The software serving a certificate is the machine's, and its version, its resolvers and its configuration marker are graded separately against the machine.

Filing per application also keeps the heal attempts and backoff of [CHK](../tamanu/healthchecks.md#self-healing) separate, so one application's stalled collection does not consume another's allowance.

Each reports one instance per name or certificate it grades, so one failing or expiring is graded and silenced without quieting the others ([CHK](../tamanu/healthchecks.md#instances)).
The numeric telemetry they declare is described in [MET](../tamanu/metrics.md).

### The collection check

The `canopy_certificates` check grades whether this server is holding the certificates it should be getting from Canopy.
What it grades is the collection: that a DNS name the server ought to have a chain for has one, and that the chain is not running down.
Obtaining and serving the chains themselves is described above, and the entitlement the check reads is described in [NAM](names.md).

#### Which DNS names the collection check grades

The check grades the DNS names [certified](#which-dns-names-are-certified) that belong to *its own* application ([NAM](names.md#which-application-a-dns-name-belongs-to)).
A site that does not name the daemon's endpoint can never be served from Canopy, so its DNS names are not graded, and a host where no site names the endpoint has nothing to grade.

An application's entry is matched to the application the check is running for by the application type, which is what Canopy puts on the wire for a reporter to correlate against; on a machine hosting a single application Canopy gives that entry as the answer itself.

The check reads the daemon's record of which DNS names Canopy last refused, how, and with what reason ([Undeclared and denied DNS names](#undeclared-and-denied-dns-names)).
It reads that record from where the daemon keeps it rather than asking the running daemon, since the record decides which DNS names go ungraded and an answer over the daemon's local interface could come from any process holding its port, where the record is held to the same trust as the chains the check grades.
Where the record cannot be read, the check grades only the DNS names Canopy's answer says this application declares, since a DNS name it does not declare may be waiting on an operator and nothing here can tell, and it says in its detail that the record could not be read.

The entitlement, Caddy's configuration, and the collected chains are each one answer for the machine, so a sweep takes each once and every application's run of the check reads the same one.
Two checks in a sweep cannot disagree about what the host serves or holds, and a machine carrying several applications costs one reading rather than one per application.

#### Collection outcomes

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

#### When the collection check skips

The check skips when its application holds no TLS grant, and while Canopy reports that application paused, naming the unmet precondition.
The reason a skip carries states only that the application is not obtaining certificates.

A grant or a pause is an application's own, so one application skipping leaves the others on the machine graded as they were.

A skip closes an issue the check had already opened, so a grant withdrawn during an incident quietens this check rather than adding to the incident.
A pause is not escalated however long it lasts, Canopy being where it was set, and Canopy is what reports a pause old enough to have let something lapse.

A revoked certificate pauses its application in Canopy ([Revocation and key replacement](#revocation-and-key-replacement)), so a revocation quietens this check for that application by the same route, and leaves the others on the machine reporting.
That is the intent: the operator who revoked the certificate is acting on the host already, and does not need this check telling them the chain they just revoked is missing.

#### How the collection check reports

The check reports one instance per DNS name it lists, keyed by the DNS name.
A graded name is passed or failed.
Its detail carries whether a chain is collected, the days remaining, whether Canopy holds a certificate for it, and the reason its order is failing, if any.
Where the refusal record could not be read, the check's detail says so and that only declared names were graded.

### The Caddy certificate check

The `caddy_certs` check grades the TLS certificates the host actually serves: that none is running out, and that what Caddy serves is what Caddy is configured to serve.
It grades certificates whatever their source, so a host obtaining chains from Canopy and a host issuing for itself are both covered.

The check runs for each Tamanu and each mSupply application on the host, and grades the certificates served for the DNS names belonging to that application.
A certificate serving DNS names belonging to several applications is graded under each of them.
A certificate serving only DNS names belonging to no application is not graded.

#### Which certificates the Caddy check grades

The certificates that matter are those Caddy's live configuration references: the managed certificates whose names are still served, and any certificate the configuration loads by hand.
Caddy's on-disk store is not the source of that list, because it keeps certificates for sites long since removed and grading those would be noise.

A renewal obtained from a different issuer than the last one is stored alongside the previous copy rather than replacing it, and only the newest is served, so a set of names is graded on the certificate that expires last.
A certificate reached by several names or from several sources is graded once.

The check skips when Caddy's configuration cannot be read, which is how a host not running Caddy is passed over, and when the configuration references no certificate the check can read for a DNS name belonging to its application.

#### Expiry

A certificate is graded on expiry only once it is inside the window its holder should have renewed it in.
Before that point no renewal has been attempted and a modest remaining life is expected, so grading it would fire on every certificate in the fleet partway through its life.

The window opens when a third of the certificate's lifetime remains.
Inside it the thresholds scale with the certificate's own lifetime rather than being fixed durations: the check warns when seven thirtieths of the lifetime remain and fails when seven ninetieths do, which is three weeks and one week for a ninety-day certificate.
A certificate's lifetime is not fixed across the fleet: a short-lived profile and a long-lived one both occur, and a fixed threshold would fire far too late for the first and far too early for the second.

#### Served against configured

The check completes a handshake against the host itself and compares the certificate served with the one the configuration and store say should be served.
A mismatch warns: it means the serving process has not picked up a certificate that has already been renewed.

The handshake is made to the host's own address so the name resolves to the local server rather than out to the internet, which is what makes the comparison about this host.

#### Certificates from Canopy

A certificate the alertd daemon serves from Canopy is graded like any other, and is reported as having come from Canopy rather than from Caddy's own issuance.

The two are distinguished because they fail differently and are fixed differently.
A host whose collection has stopped working falls back to Caddy's own issuance and keeps serving, so without the distinction it presents exactly as a healthy Canopy-served host while still depending on the DNS credential that issuing through Canopy exists to remove.
Whether the collection itself is working is graded separately by [the collection check](#the-collection-check).

#### How the Caddy check reports

The check reports one instance per certificate it grades.
An instance is keyed by the certificate's DNS names, sorted and joined, or by where it was loaded from when it names none, so the key stays the same across renewals.
Its label is the DNS names.
An instance is passed, warning or failed by its own expiry and served-against-configured grades, the worse of the two, and its detail carries the DNS names, the source, where it was loaded from, when it expires, the days remaining, its lifetime, and whether the served certificate matched.

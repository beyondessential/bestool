---
id: CHK-CCT
---

# The Caddy certificate check

The `caddy_certs` check grades the TLS certificates the host actually serves: that none is running out, and that what Caddy serves is what Caddy is configured to serve.
It is one of the doctor's healthchecks; see [DOC](doctor.md) for the framework it runs in and [CHK](healthchecks.md) for how its outcome is reported.

It grades certificates whatever their source, so a host obtaining chains from Canopy ([TLS](../canopy/certificates.md)) and a host issuing for itself are both covered.

The check reports for an application rather than for the machine ([SUBJ](subjects.md)): a certificate is issued for the names an application answers on, and it is that application's group that answers for one running out.
The software serving it is the machine's, and its version, its resolvers and its configuration marker are graded separately against the machine.

## Which certificates it grades

The certificates that matter are those Caddy's live configuration references: the managed certificates whose names are still served, and any certificate the configuration loads by hand.
Caddy's on-disk store is not the source of that list, because it keeps certificates for sites long since removed and grading those would be noise.

A renewal obtained from a different issuer than the last one is stored alongside the previous copy rather than replacing it, and only the newest is served, so a set of names is graded on the certificate that expires last.
A certificate reached by several names or from several sources is graded once.

The check skips when Caddy's configuration cannot be read, which is how a host not running Caddy is passed over, and when the configuration references no certificate the check can read.

## Expiry

A certificate is graded on expiry only once it is inside the window its holder should have renewed it in.
Before that point no renewal has been attempted and a modest remaining life is expected, so grading it would fire on every certificate in the fleet partway through its life.

Inside that window the thresholds scale with the certificate's own lifetime rather than being fixed durations, warning while there is room to recover and failing as the remaining life runs down.
A certificate's lifetime is not fixed across the fleet: a short-lived profile and a long-lived one both occur, and a fixed threshold would fire far too late for the first and far too early for the second.

## Served against configured

The check completes a handshake against the host itself and compares the certificate served with the one the configuration and store say should be served.
A mismatch warns: it means the serving process has not picked up a certificate that has already been renewed.

The handshake is made to the host's own address so the name resolves to the local server rather than out to the internet, which is what makes the comparison about this host.

## Certificates from Canopy

A certificate the alertd daemon serves from Canopy ([TLSD](../canopy/certificate-delivery.md)) is graded like any other, and is reported as having come from Canopy rather than from Caddy's own issuance.

The two are distinguished because they fail differently and are fixed differently.
A host whose collection has stopped working falls back to Caddy's own issuance and keeps serving, so without the distinction it presents exactly as a healthy Canopy-served host while still depending on the DNS credential that issuing through Canopy exists to remove.
Whether the collection itself is working is graded separately by [CHK-CCO](../canopy/certificate-collection-check.md).

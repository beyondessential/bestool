---
id: CHK-DTR
---

# Distrusted root certificates

The `distrusted_roots` healthcheck grades whether the machine, or the Tamanu deployed on it, still trusts a root certificate authority that has been distrusted.
It is one of the doctor's healthchecks; see [CHK](healthchecks.md) for the framework it runs in.
It reports for the machine, as described in [SUBJ](subjects.md).

> [!NOTE]
> Neither the host's trust store nor Node.js's built-in one is checked against revocation information, so a root whose intermediates have been compromised stays trusted until it is removed from each store that carries it.
> Root programs remove such a root in an emergency, but a host or a container image only follows once it is updated.
> This check shows which machines still trust one, so attention goes where it is needed.

## Distrusted roots

The check carries a fixed list of distrusted roots, each identified by the SHA-256 fingerprint of its certificate and named for display.
The list is:

- emSign ECC Root CA - G3, fingerprint `86:A1:EC:BA:08:9C:4A:8D:3B:BE:27:34:C6:12:BA:34:1D:81:3E:04:3C:F9:E8:A8:62:CD:5C:57:A3:6B:BE:6B`.

> [!NOTE]
> emSign ECC Root CA - G3 is distrusted because the private keys of TLS intermediates under it can be recovered by anyone (Mozilla bug 2083449).

## Trust stores

The check reads every trust store that TLS connections from the machine or its Tamanu verify against:

- The host's own trust store.
  On Windows, a root counts as trusted unless it is in the machine's untrusted certificates store, because Windows fetches roots from its root program as they are needed, so a root missing from the local store may still be trusted.
- When a Tamanu server container is running on the machine, the default trust store of the Node.js runtime inside it, as a process started in that container sees it, and the container's system trust bundle at `/etc/ssl/certs/ca-certificates.crt`.
- When a Tamanu deployment on the machine has a bundled Node.js runtime, the default trust store of that runtime.

A Tamanu trust store is absent, rather than unreadable, when the machine has no Tamanu or no running server container or bundled runtime to read it from.
It is unreadable when the machine's containers or the server container's system trust bundle could not be read.

A trust store that holds no certificates, or that could only be read in part, counts as unreadable, unless what was read already trusts a distrusted root.

## Outcome

- Failed when any trust store read trusts a distrusted root.
  The reason names each distrusted root that is trusted and the trust stores trusting it.
- Warning when no trust store read trusts a distrusted root but one or more could not be read.
  The reason names the trust stores that could not be read.
- Passed otherwise.

The check's details list, for each trust store, the distrusted roots it trusts, or why it could not be read.

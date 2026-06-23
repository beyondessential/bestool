---
id: S3P
---

# S3 SigV4 re-signing proxy

A small loopback HTTP proxy that fronts a real S3 endpoint and re-signs every request with live, auto-refreshing credentials, so a long-running S3 client that binds its credentials once at start-up can keep talking to S3 past the lifetime of any single set of credentials. It is a standalone, publishable crate so both Canopy (server-side maintenance, inspection, init) and bestool (device backups and restores) drive kopia through the same mechanism.

## Why it exists

kopia's S3 connector resolves credentials once, at process start, and signs every request for the life of the process with that fixed key material. It has no mid-run refresh: its CLI requires real `--access-key`/`--secret-access-key` at parse time, and it does **not** consume an ECS-style container-credentials endpoint for the S3 backend (verified against kopia 0.23.1 — it errors on the missing flags before it would ever poll the endpoint). Assumed-role / STS credentials are short-lived (about an hour), so any kopia operation that outlives the credentials fails partway through. That window is routinely exceeded: maintenance on a mid-size repository has run 40 minutes and climbing, and device backups and restores of large clusters run much longer.

Static credentials passed by environment are an adequate stopgap for genuinely short operations (init, stats, listing snapshots, rotation, inspection), but not for these. The proxy removes the time bound: kopia holds only meaningless static dummy keys and points at the loopback proxy; the proxy holds the live credentials, refreshes them transparently, and re-signs each request as it passes through. A run is then limited by how long Canopy stays reachable to reissue credentials, not by the lifetime of one issuance.

This supersedes, for the S3 backend, the container-credentials endpoint described in [BAK](backup.md); that endpoint should be retired once this lands.

## Shape

kopia is configured to use S3 against the proxy: a loopback endpoint (`127.0.0.1:<ephemeral-port>`), TLS disabled on that leg, and fixed dummy access/secret keys. The proxy accepts those requests, discards their dummy signature, re-signs the request with the current real credentials for the true region and the `s3` service, and forwards it to the real S3 host over TLS. The response is streamed back verbatim. Each request is signed independently with whatever credentials are current at that moment, so a credential refresh between two requests is invisible to kopia.

The proxy is a forward proxy in spirit but a re-signing reverse proxy in mechanics: it owns the upstream target (bucket host, region) and the credentials; kopia owns nothing but the dummy keys and the loopback address.

## Credentials are pluggable

The credential source is the one integration seam. The crate defines a provider abstraction that yields the current access key, secret key, and session token, and refreshes itself ahead of expiry; the proxy asks the provider for current credentials per request (cheap when unexpired) and never blocks the request path on a network round-trip it can avoid.

- **Canopy** plugs an in-process assume-role provider (the pod assumes the group's role; the AWS SDK refreshes the underlying identity), so a run is not capped at the chained-session ceiling.
- **bestool** plugs a provider backed by Canopy's issue-credentials endpoint, fetching on first use and again as expiry approaches, for the `backup` (write-without-delete) or `restore` (read-only) purpose.

The crate carries neither Canopy nor bestool specifics beyond this trait.

## Signing correctness is the hard part

The proxy must reproduce a correct SigV4 signature for each forwarded request, which means it must handle however kopia's S3 client (minio-go) chooses to sign request bodies — in particular streaming uploads. minio-go signs large PUTs over a non-TLS endpoint with a streaming, chunked payload signature (`STREAMING-AWS4-HMAC-SHA256-PAYLOAD`), where per-chunk signatures embedded in the body are keyed to the signing key. The proxy cannot simply swap the `Authorization` header and pass the body through: the in-body chunk signatures were computed with the dummy key and would not verify upstream.

The proxy must therefore either re-derive the streaming chunk signatures with the live key as it streams the body, or coerce the client into a payload mode it can re-sign by header alone (e.g. unsigned payload over the trusted loopback leg, signed afresh upstream over TLS). Either way, request and response bodies must stream — large objects must not be buffered whole. **This is the primary technical risk and must be validated with a spike before the rest is built;** the choice of approach should be settled by what minio-go can actually be made to emit and what S3 accepts, not assumed. Whatever bound is chosen (max in-flight size, buffering threshold), it must be explicit, not silent.

## Lifecycle and concurrency

The proxy binds an ephemeral loopback port and lives for the operation it serves. bestool runs one per backup or restore run; Canopy's backups pod runs many groups concurrently, each with its own role, bucket, and region, so the crate must support cheap, independent instances (a proxy per op, each with its own provider and upstream target) rather than a single shared singleton — or, if multiplexed, must key cleanly on the per-op target and credentials. Spawning and tearing one down per op must be inexpensive.

## Security

The proxy binds a loopback literal only and is never exposed off-host. The dummy keys kopia carries are meaningless on their own; the real credentials live only in the proxy's process memory and are never written to disk or logged. Ambient AWS environment variables that could let the host's own credentials shadow the dummy keys are scrubbed from kopia's environment, as today. The loopback leg runs without TLS (trusted, same host); the upstream leg to S3 is always TLS.

## Observability

The proxy logs credential refreshes, upstream S3 errors, and lifecycle (bind, shutdown), but never credential material. A refresh failure (Canopy unreachable, assume-role denied) surfaces as a failed request to kopia with enough context to distinguish "lost credentials mid-run" from an ordinary S3 error.

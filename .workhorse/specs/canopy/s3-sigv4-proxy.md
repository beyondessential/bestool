---
id: S3P
---

# S3 SigV4 re-signing proxy

A small loopback HTTP proxy that fronts a real S3 endpoint and re-signs every request with live, auto-refreshing credentials, so a long-running S3 client that binds its credentials once at start-up can keep talking to S3 past the lifetime of any single set of credentials.
Both Canopy (server-side maintenance, inspection, init) and bestool (device backups and restores) drive kopia through it.

## Why it exists

kopia's S3 connector resolves credentials once, at process start, and signs every request for the life of the process with that fixed key material.
It has no mid-run refresh: it binds real credentials at start-up and has no way to pick up fresh ones for the S3 backend while running.
Assumed-role / STS credentials are short-lived (about an hour), so any kopia operation that outlives the credentials fails partway through.
That window is routinely exceeded: maintenance on a mid-size repository has run 40 minutes and climbing, and device backups and restores of large clusters run much longer.

Static credentials passed by environment are an adequate stopgap for genuinely short operations (init, stats, listing snapshots, rotation, inspection), but not for these.
The proxy removes the time bound: kopia holds only meaningless static dummy keys and points at the loopback proxy; the proxy holds the live credentials, refreshes them transparently, and re-signs each request as it passes through.
A run is then limited by how long Canopy stays reachable to reissue credentials, not by the lifetime of one issuance.

## Shape

kopia is configured to use S3 against the proxy: a loopback endpoint (`127.0.0.1:<ephemeral-port>`), TLS disabled on that leg, and fixed dummy access/secret keys.
The proxy accepts those requests, discards their dummy signature, re-signs the request with the current real credentials for the true region and the `s3` service, and forwards it to the real S3 host over TLS.
The response is streamed back verbatim.
Each request is signed independently with whatever credentials are current at that moment, so a credential refresh between two requests is invisible to kopia.

The proxy is a forward proxy in spirit but a re-signing reverse proxy in mechanics: it owns the upstream target (bucket host, region) and the credentials; kopia owns nothing but the dummy keys and the loopback address.

## Credentials are pluggable

The credential source is the one integration seam.
Credentials come from a pluggable provider that yields the current access key, secret key, and session token and refreshes itself ahead of expiry; the proxy asks the provider for current credentials per request (cheap when unexpired) and never blocks the request path on a network round-trip it can avoid.

- **Canopy** plugs an in-process assume-role provider (the pod assumes the group's role and refreshes the underlying identity), so a run is not capped at the chained-session ceiling.
- **bestool** plugs a provider backed by Canopy's issue-credentials endpoint, fetching on first use and again as expiry approaches, for the `backup` (write-without-delete) or `restore` (read-only) purpose.

The proxy carries neither Canopy nor bestool specifics beyond this seam.

## Signing

Every object PUT arrives over the loopback leg with a chunked streaming payload signature (`STREAMING-AWS4-HMAC-SHA256-PAYLOAD`): the body is a sequence of chunks, each prefixed on the wire by a per-chunk signature chained from the request's seed signature and keyed to the signing key.
The proxy re-derives this whole chain with the live signing key; swapping the `Authorization` header alone does not suffice, as the in-body chunk signatures are keyed to the credentials too.

For each PUT the proxy computes a fresh seed signature for the re-signed request, derives the signing key from the live credentials, then walks the body chunk by chunk recomputing each chunk signature — `HMAC-SHA256(signing_key, "AWS4-HMAC-SHA256-PAYLOAD\n" + amz_date + "\n" + scope + "\n" + previous_signature + "\n" + SHA256("") + "\n" + SHA256(chunk_data))`, seeded by the request signature and chained through to the terminating zero-length chunk.
Chunk sizes are copied from the incoming framing verbatim, so the re-encoded body is the same byte length and `Content-Length` is unchanged.

GET, HEAD and DELETE carry no body and are re-signed by recomputing the `Authorization` header alone.
The proxy handles single-object PUT, GET, HEAD, DELETE and bucket listing — the operations kopia performs; multipart upload is out of scope, kopia's pack blobs being small enough to send as single PUTs.

Bodies stream both ways — the proxy never holds a whole object.
Chunks are self-framing, so it parses and re-emits the request body one chunk at a time; the only buffer is the current chunk, capped at an explicit ceiling far above kopia's 64 KiB so an unexpectedly large chunk (or an unterminated chunk header) is rejected rather than buffered without bound.
Responses are streamed through verbatim.

The signed header set is `host`, `x-amz-date`, `x-amz-content-sha256`, and `x-amz-decoded-content-length` on streaming PUTs; for STS credentials `x-amz-security-token` is added before signing so it is covered by the signature; `content-type`, `content-md5` and `content-encoding` are signed when present.
A credential refresh between two requests changes only the key material the next request is signed with — kopia's view does not change.

## Network path

The signature binds a request to a host and region, not to a route, so the address the proxy connects to is a separate axis from what it signs: the connection target and the signed host/region are independent settings.
This leaves the egress path open without touching how requests are signed — S3 can be reached directly, over a Tailscale subnet router into a VPC, or through an S3 interface endpoint (PrivateLink) with private DNS, all while the signed host and region stay the canonical S3 values.
The only constraint is that whatever finally reaches S3 carries the host and signature S3 validates: an L3 route that forwards the request unchanged is transparent; a hop that terminates TLS must preserve the `Host` header or re-sign.
The upstream TLS leg validates the endpoint's certificate against the host the proxy connects to, so a connection target whose name differs from the signed host needs a certificate that covers it.

## Lifecycle and concurrency

The proxy binds an ephemeral loopback port and lives for the operation it serves.
bestool runs one per backup or restore run; Canopy's backups pod runs many groups concurrently, each with its own role, bucket, and region, so the proxy must support cheap, independent instances (one per op, each with its own provider and upstream target) rather than a single shared singleton — or, if multiplexed, must key cleanly on the per-op target and credentials.
Spawning and tearing one down per op must be inexpensive.



## Security

The proxy binds a loopback literal only and is never exposed off-host.
The dummy keys kopia carries are meaningless on their own; the real credentials live only in the proxy's process memory and are never written to disk or logged.
Ambient AWS environment variables that could let the host's own credentials shadow the dummy keys are scrubbed from kopia's environment.
The loopback leg runs without TLS (trusted, same host); the upstream leg to S3 is always TLS.

## Observability

The proxy logs credential refreshes, upstream S3 errors, and lifecycle (bind, shutdown), but never credential material.
A refresh failure (Canopy unreachable, assume-role denied) surfaces as a failed request to kopia with enough context to distinguish "lost credentials mid-run" from an ordinary S3 error.

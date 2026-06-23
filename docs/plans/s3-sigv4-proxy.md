# S3 SigV4 re-signing proxy

Implements [S3P](../../.workhorse/specs/canopy/s3-sigv4-proxy.md). The signing
approach is settled and validated end-to-end against AWS S3 with real STS
credentials (full kopia lifecycle, byte-identical restore); this plan is the
production build. The validated throwaway proxy (buffered, single-file) lives at
`~/sigv4-spike/` for reference — the production version is the same signing
maths made streaming and pluggable.

## Context

kopia binds its S3 credentials once at start-up; long operations (maintenance,
large backups/restores) outlive STS credentials. The proxy sits on a loopback
port: kopia talks to it over plain HTTP with dummy keys, and it re-signs each
request with live credentials and forwards to real S3 over TLS. Today's
container-credentials endpoint (`crates/bestool/src/actions/canopy/backup/creds.rs`)
is the thing this retires for the S3 backend.

Key facts the build relies on (verified, kopia 0.23.1 / minio-go):
- Over a non-TLS endpoint, minio-go signs **every** object PUT with the chunked
  `STREAMING-AWS4-HMAC-SHA256-PAYLOAD` body; no kopia-exposed option disables it.
- GET/HEAD/DELETE/bucket-list carry no body. No multipart upload occurs (packs
  are ~20 MB single PUTs).
- The re-encoded chunk body is byte-identical in length, so `Content-Length` is
  unchanged.

## Crate layout

The proxy goes in `bestool-kopia` (`crates/kopia`) behind a new cargo feature
`proxy` (alongside the existing `cli` feature), so `alertd` and `tamanu` — which
use only the snapshot helpers — don't pull in async/TLS deps. Feature `proxy`
pulls: `tokio`, `hyper` + `hyper-util` + `hyper-rustls` + `http-body-util`,
`hmac`, `sha2`, `hex` (all already in the workspace lockfile via reqwest/axum).

Modules (newer `foo.rs` + `foo/sub.rs` style):
- `src/proxy.rs` — public API: `S3Proxy`, `S3ProxyConfig`, `RunningProxy`, and
  the `CredentialProvider` trait + `Credentials`.
- `src/proxy/sigv4.rs` — signing key derivation, canonical request, seed
  signature, per-chunk signature.
- `src/proxy/stream.rs` — streaming chunk parse + re-encode `http_body::Body`.
- `src/proxy/server.rs` — hyper accept loop + per-request handler.

## CredentialProvider trait

The one integration seam. Object-safe, async, no new dep (hand-rolled boxed
future to avoid `async-trait`):

```rust
pub struct Credentials {
    pub access_key: String,
    pub secret_key: String,
    pub session_token: Option<String>,
}

pub trait CredentialProvider: Send + Sync {
    /// Current credentials. Cheap when unexpired; refreshes ahead of expiry
    /// internally. Must not block the request path on an avoidable round-trip.
    fn credentials(&self)
        -> Pin<Box<dyn Future<Output = Result<Credentials>> + Send + '_>>;
}
```

- bestool: `CanopyCredentialProvider` backed by Canopy's issue-credentials
  endpoint, refactored out of `creds.rs` (the fetch/refresh-margin logic is
  already there). Caches, refreshes ~2 min before expiry, for the `backup`
  (write-without-delete) or `restore` (read-only) purpose.
- Canopy server (separate repo): plugs an in-process assume-role provider. Out
  of scope here — this crate only exposes the trait.
- A `StaticCredentialProvider` lives in the crate for tests and short-op reuse.

## SigV4 signing (`sigv4.rs`)

Port the validated spike maths:
- `signing_key(secret, date_stamp, region, "s3")` — the 4-step HMAC chain.
- `canonical_uri` / `canonical_query` — percent-encode unreserved + `/` for the
  path, unreserved-only for query, sorted.
- Seed signature: canonical request with hashed-payload =
  `STREAMING-AWS4-HMAC-SHA256-PAYLOAD` (PUT) or the incoming
  `x-amz-content-sha256` value (header-only ops).
- Signed header set: `host`, `x-amz-date`, `x-amz-content-sha256`,
  `x-amz-decoded-content-length` (streaming PUTs), `x-amz-security-token` (STS),
  and `content-type`/`content-md5`/`content-encoding` when present. Reuse the
  incoming `x-amz-date` so scope/date stay consistent.
- `chunk_sign(signing_key, amz_date, scope, prev, data)` — the
  `AWS4-HMAC-SHA256-PAYLOAD` string-to-sign.

**Tests**: unit-test canonicalisation + signature against the published AWS
SigV4 test-suite vectors (real AWS is strict; lock this down independently of a
live server). Unit-test the chunk-signature chain seeding + termination.

## Streaming body re-encode (`stream.rs`)

An `http_body::Body` wrapping the incoming request body that transforms chunk
framing on the fly: read a `<hexsize>;chunk-signature=<64hex>\r\n` header, read
`hexsize` bytes of data, re-emit with a recomputed signature, chained to the
terminating zero chunk. Only the current chunk is buffered (64 KiB). `size_hint`
returns the exact encoded length (= incoming `Content-Length`, since framing is
preserved), so hyper sends a sized body, not chunked transfer-encoding — which
streaming SigV4 requires. Handle a chunk split across TCP reads.

## Request handler (`server.rs`)

Per request: fetch `provider.credentials()`; read method, path, query, headers;
pick streaming vs header-only from `x-amz-content-sha256`; build forwarded
headers (drop `host`/`authorization`/`content-length`/`connection`/
`accept-encoding`; set upstream `host`; add `x-amz-security-token` if present);
compute Authorization; for PUT wrap the body in the re-signing `Body`, else pass
through; forward via a `hyper-util` client with a `hyper-rustls` HTTPS connector;
stream the response back verbatim. A credential-fetch failure returns a
distinguishable error so kopia's failure says "lost credentials" vs an ordinary
S3 error.

## Lifecycle (`proxy.rs`)

`S3Proxy::spawn(config, provider) -> Result<RunningProxy>` binds `127.0.0.1:0`,
returns the ephemeral `SocketAddr` and a handle whose `Drop` (or explicit
`shutdown().await`) stops the accept loop. `S3ProxyConfig { upstream_host,
region }`. Cheap per-op: bestool spawns one per backup/restore; instances are
independent, each with its own provider and upstream target.

## Integration (bestool)

1. `args_repository_connect_s3` (`crates/kopia/src/lib.rs:286`): add params to
   point kopia at the proxy — `--endpoint 127.0.0.1:<port>`, `--disable-tls`,
   dummy `--access-key`/`--secret-access-key`. (Region/bucket/prefix stay.)
2. `S3KopiaEnv` / `build_kopia_command_with_s3` (`crates/kopia/src/lib.rs:218`):
   drop the `AWS_CONTAINER_CREDENTIALS_FULL_URI` / `AWS_CONTAINER_AUTHORIZATION_TOKEN`
   vars. Keep `KOPIA_PASSWORD`/`KOPIA_CONFIG_PATH`. Keep the `S3_SHADOWING_ENV_VARS`
   scrub (now so ambient AWS creds can't shadow the dummy keys at the proxy) and
   update its doc comment.
3. `crates/bestool/src/actions/canopy/backup.rs` + `restore.rs`: spawn the proxy,
   build a `CanopyCredentialProvider`, pass the proxy addr into the connect args,
   keep it alive for the run.
4. Retire `crates/bestool/src/actions/canopy/backup/creds.rs` (`CredsServer`,
   `CredsLease`, handler) once nothing references it; migrate its fetch/refresh
   logic into `CanopyCredentialProvider`. Update tests.

## Observability

`tracing`: credential refreshes (never the material — log access-key id at most),
upstream non-2xx, bind/shutdown. Refresh failure vs S3 error distinguishable in
the error surfaced to kopia.

## Integration test (CI)

A GHA job `kopia-proxy` in `.github/workflows/ci.yml` reproduces the spike
end-to-end and is added to the `tests-pass` `needs` list:
- start MinIO with `docker run -d` (it needs a `server /data` command, which
  `services:` containers can't supply), wait for `/minio/health/live`, create a
  bucket with the runner's preinstalled aws cli;
- download the kopia 0.23.1 release binary;
- run the gated test with endpoint/creds/kopia-path in env:
  `cargo test -p bestool-kopia --features proxy --test proxy_e2e -- --ignored`.

The test (`crates/kopia/tests/proxy_e2e.rs`, `#[ignore]`) spawns the proxy with a
`StaticCredentialProvider` holding the MinIO creds, then drives
`repository create` → `snapshot create` (with a >64 KiB incompressible file to
force a multi-chunk streaming PUT) → `snapshot list` → `snapshot restore` →
`maintenance run --full` via `std::process::Command` with dummy keys pointed at
the proxy, and asserts the restore is byte-identical and kopia exits 0
throughout. It skips cleanly when the env (MinIO endpoint, kopia path) is absent,
so a bare `cargo test` on a dev box is unaffected — the `DATABASE_URL` /
`canopy_contract` pattern.

## Commit sequence

1. `proxy` feature + crate deps; `sigv4.rs` + vector tests.
2. `stream.rs` + chunk tests.
3. `server.rs` + `proxy.rs` (spawn/shutdown); `tests/proxy_e2e.rs` + the
   `kopia-proxy` CI job.
4. `CanopyCredentialProvider` (migrate from `creds.rs`).
5. kopia command-builder changes + wire into backup/restore.
6. Retire `creds.rs`; update USAGE.md if help text changed (`./update-usage.sh`).

## Decisions (settled)

- **HTTP stack**: hyper + hyper-util + hyper-rustls — full control of streaming
  bodies and exact `Content-Length`, which the SigV4 streaming upload needs and
  which `reqwest`'s stream body doesn't give cleanly. The spike used axum +
  reqwest with whole-body buffering; production streams.
- **Integration landing**: land the crate (steps 1–3) self-contained and tested
  first, then the bestool wiring (4–6) on top, so the proxy is provable in
  isolation before anything depends on it.

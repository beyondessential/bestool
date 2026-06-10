# bestool canopy backup-credentials / backup

Implementation spec for the device-side half of Canopy's backup-credentials
system. This is the bestool side; the Canopy side (endpoints, DB, IAM, IaC,
staleness detection) lives in the canopy repo's `docs/plans/backup-credentials.md`
and is the authoritative contract this spec consumes.

The short version: Canopy issues short-lived per-group S3 credentials to remote
servers so kopia backups run without each server holding long-lived bucket keys.
bestool is the device-side actor: it fetches the target, fetches credentials (as
a kopia/AWS-SDK `credential_process` shim), drives kopia, and reports the
outcome. Canopy owns scheduling, retention, maintenance, inspection, and
alerting; the device never deletes and never caches the bucket.

## Purpose

Two new subcommands under the existing `bestool canopy` group:

- **`bestool canopy backup-credentials [--purpose backup|restore]`** — a
  `credential_process` shim. POSTs `/backup-credentials` over the device mTLS
  identity, writes the AWS-SDK `credential_process` JSON to stdout verbatim,
  exits 0 on success and non-zero on any failure.

- **`bestool canopy backup [--purpose backup|restore]`** — the driver. On
  *every run*: `GET /backup-target` to learn `{storage, bucket, prefix, region,
  repo_password}`, reconcile the kopia repository connection against it (wiring
  `credential_process = bestool canopy backup-credentials`), run the backup (or
  restore), then `POST /backup-report`. Mints the run-uuid at run start (it
  becomes `backup_runs.id`), sets the kopia source hostname to the server id,
  and tags snapshots with `canopy-device` / `canopy-run`.

Neither command is ever provisioned with a bucket name. The device holds only
its Canopy URL and mTLS identity (in the existing encrypted `Registration`);
target and credentials are re-derived from Canopy on each run. A server-side
config change therefore propagates fleet-wide with no per-host action.

## Where this lives in the repo

Follow the established `canopy` subcommand layout (mirrors `register` / `export`
/ `import`):

```
crates/bestool/src/actions/canopy.rs                    # add two variants to the subcommands! block
crates/bestool/src/actions/canopy/backup_credentials.rs # the credential_process shim (NEW)
crates/bestool/src/actions/canopy/backup.rs             # the driver (NEW)
crates/canopy/src/client.rs                             # add backup_credentials() / backup_target() / backup_report() to CanopyClient
crates/canopy/src/lib.rs                                # re-export the new request/response types
```

Client transport, mTLS, tailscale-vs-public fallback, and cert renewal already
exist on `bestool_canopy::CanopyClient` (`crates/canopy/src/client.rs`). Add the
three calls there rather than building ad-hoc reqwest clients in the action
modules. The `register` action builds its own transport because enrollment
predates having a device identity to reuse; backup commands run on an
already-enrolled host, so they should construct a `CanopyClient` exactly as
`alertd`'s daemon does (`crates/alertd/src/daemon.rs:33`): load the
`Registration`, pass `device_key`, prefer tailscale, fall back to mTLS.

kopia invocation reuses the `bestool-kopia` crate: `find_kopia_binary`,
`build_kopia_command` (Linux `sudo -u kopia` elevation), `Snapshot` /
`fetch_snapshots` for parsing. The crate currently has no
"repository connect" / "snapshot create" helpers — those are net-new (see
"kopia invocation" below); put generic, non-canopy-specific ones in
`bestool-kopia` and canopy-specific wiring in the action module.

### Cargo features

Extend the existing pattern in `crates/bestool/Cargo.toml`:

```toml
canopy = [ "canopy-register", "canopy-export", "canopy-import",
           "canopy-backup-credentials", "canopy-backup" ]
canopy-backup-credentials = ["__canopy", "bestool-tamanu/canopy-registration"]
canopy-backup            = ["__canopy", "bestool-tamanu/canopy-registration", "dep:bestool-kopia"]
```

`canopy-backup-credentials` needs no kopia dep (it only talks HTTP and prints
JSON); `canopy-backup` pulls in `bestool-kopia`. Gate the two action modules and
their `subcommands!` variants with `#[cfg(feature = "...")]` exactly as
`register`/`export`/`import` are gated. Don't pull `dep:p256`/`dep:algae-cli`
into these — they don't enroll or decrypt tickets.

## Subcommand wiring

In `crates/bestool/src/actions/canopy.rs`, add to the `subcommands!` block:

```rust
#[cfg(feature = "canopy-backup-credentials")]
backup_credentials => BackupCredentials(BackupCredentialsArgs),
#[cfg(feature = "canopy-backup")]
backup => Backup(BackupArgs)
```

Each leaf `run(args, ctx)` extracts the top-level `Args` via `ctx.require()` if
it needs global flags (verbosity, etc.); both load the `Registration` with the
existing migration-aware loader (`super::load_registration` / `registration::load`).
The `--config <DIR>` flag should be accepted on both, matching `register`/`export`,
so tests and ad-hoc relocation work via `BESTOOL_CANOPY_DIR` too.

### Shared `--purpose`

```rust
#[derive(Debug, Clone, Copy, clap::ValueEnum, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Purpose { Backup, Restore } // default Backup
```

Define once (e.g. in `canopy.rs` or a small `backup_common.rs`) and reuse in
both args structs. Serializes to `"backup"` / `"restore"` for the request
bodies. Default is `Backup`.

## `bestool canopy backup-credentials` (the credential_process shim)

This is what kopia/the AWS SDK execs to obtain creds. Contract: stdout is the
fixed AWS `credential_process` JSON and nothing else; any non-zero exit means
"creds unavailable" to the SDK.

Flow:

1. Load `Registration`; require `device_key` + `api_url`. If unregistered, exit
   non-zero with a stderr message (never partial JSON on stdout).
2. Build a `CanopyClient` (tailscale-preferred, mTLS fallback).
3. POST `/backup-credentials` with body `{ "purpose": <purpose> }`.
4. On `200`, **write the response body to stdout verbatim** and exit 0. The body
   is already exactly the SDK shape; do not reserialize through a typed struct
   that might reorder/normalize fields. (A typed `BackupCredentials` may exist
   for tests/validation, but the bytes printed should be Canopy's response body
   unchanged — relay, don't transform.)
5. On any non-2xx or transport error, print a diagnostic to **stderr** and exit
   non-zero. Stdout must stay empty so the SDK sees no credentials rather than
   garbage.

Exit-code contract (the SDK only distinguishes zero from non-zero, but be
deliberate so logs are useful):

| Situation | Exit | Stdout |
|---|---|---|
| `200` creds returned | 0 | the JSON, verbatim |
| `412` device not bound to a live server | non-zero | empty |
| `409` ungrouped / no backup config | non-zero | empty |
| `502` STS failed upstream | non-zero | empty |
| transport / TLS / unregistered | non-zero | empty |

Note the dormant cases (`412`/`409`) are *non-zero* here. They are "benign
dormant" only at the `backup` driver level (see below); a bare
`backup-credentials` invocation that can't get creds is simply a failure to the
SDK, which is correct — kopia shouldn't proceed with no creds. The driver is
responsible for not *treating* dormancy as an error condition; it does that by
checking `/backup-target` first and never reaching the kopia step when dormant.

Output shape (fixed by the AWS SDK, do not deviate):

```json
{
  "Version": 1,
  "AccessKeyId": "...",
  "SecretAccessKey": "...",
  "SessionToken": "...",
  "Expiration": "2026-05-21T13:00:00Z"
}
```

`Expiration` is RFC3339/ISO-8601 UTC. The SDK caches creds until shortly before
this instant, then re-execs us. Because Canopy chains from its IRSA session,
expiry is capped at ~1h regardless of role config — fine, the SDK refreshes on
demand, but it means Canopy must be reachable for the *lifetime* of a backup,
not just at the start. No retry/backoff logic needed in the shim; the SDK
re-execs.

## `bestool canopy backup` (the driver)

Owns the kopia invocation so the device holds no hardcoded bucket. Runs to
completion synchronously (it's launched per-run, not a daemon — see "triggering"
below).

Flow:

1. **Mint the run-uuid** (`Uuid::new_v4()`) at the very start. This becomes
   `backup_runs.id` and is stamped into the snapshot tag `canopy-run` *before*
   any Canopy row exists. Hold it for the whole run.
2. Load `Registration`; require `device_key`, `api_url`, **`server_id`**, and
   `device_id`. `server_id` is needed for the kopia source hostname;
   `device_id` for the `canopy-device` tag. Unregistered → error exit.
3. Build a `CanopyClient`.
4. **`GET /backup-target`.**
   - `200` → proceed with `{storage, bucket, prefix, region, repo_password}`.
   - `412` / `409` → **benign dormant.** Log at info ("nothing to do: device
     not yet authorized for backups") and **exit 0**. This is the
     provision-then-authorize state: the image ships backup wiring
     unconditionally and simply idles until an operator configures the group.
     It must not be a failure, must not alert, and must not POST a report.
   - other non-2xx / transport error → real failure, exit non-zero.
5. **Reconcile the kopia repository connection** against the target (see "kopia
   invocation"). The `credential_process` is `bestool canopy backup-credentials
   --purpose <purpose>` (resolve the current exe path; pass `--config` through
   if one was given). The repo password comes from `/backup-target`'s
   `repo_password` (passed to kopia via `KOPIA_PASSWORD` env, not argv).
6. **Run** the operation:
   - `backup`: `kopia snapshot create <path>` with `--tags canopy-device=<device-id>`
     and `--tags canopy-run=<run-uuid>`, source host overridden to the server id.
   - `restore`: the read-only path (see open questions on what exactly restore
     does end-to-end). At minimum it connects read-only and verifies; restoring
     to disk is operator-directed.
7. **`POST /backup-report`** with the run-uuid and outcome. Always report for a
   run that *started* kopia (success or failure), so a crashed/failed run is not
   silent. (A run that exited at step 4 dormant did not start and reports
   nothing.)
   - If kopia succeeded: `outcome: "success"`, plus `bytes_uploaded` and
     `snapshot_id` if parseable from kopia output.
   - If kopia failed: `outcome: "failure"`, `error` = a trimmed kopia stderr /
     message. Then exit non-zero.
   - If the *report POST itself* fails, log it but the run already happened;
     exit non-zero so the operator/service notices, but do not retry forever
     (Canopy's signal-2 repository inspection is the backstop for a lost
     report). The exit code reflects the report failure, not the backup.

### kopia invocation (net-new)

`bestool-kopia` today only *reads* (`fetch_snapshots`) and locates binaries; it
has no connect/create. Add helpers there (generic) and wire them here:

- **Connect / reconcile:** `kopia repository connect s3 --bucket <bucket>
  --prefix <prefix> --region <region> --endpoint <...>? --credentials-file? ...`.
  Use kopia's S3 backend with `credential_process` for AWS creds. Two viable
  shapes, to be confirmed against the installed kopia (open question):
  - kopia reading creds via the AWS SDK chain, with our shim wired as the SDK
    `credential_process` in an `AWS_CONFIG_FILE` we render per run; or
  - kopia's own `--credentials-file` / role mechanism.
  The repo password is supplied via `KOPIA_PASSWORD`. "Reconcile" means: if
  already connected to a *different* bucket/prefix, reconnect to the target from
  `/backup-target` so a server-side bucket change is picked up here. Never write
  the bucket/prefix/region to persistent device config.
- **Source host override:** kopia derives the snapshot source from
  `user@host:path`. Set the host to the **server id** so the source is the
  backup *subject* and survives device replacement (continuous history). kopia
  supports `--override-hostname` on `repository connect` (confirm flag name
  against the installed version). The result is sources shaped
  `canopy@<server-id>:<path>`, which Canopy's inspection job parses.
- **Snapshot create with tags:** `kopia snapshot create --tags canopy-device=<uuid>
  --tags canopy-run=<run-uuid> <path>`. Parse the resulting snapshot/manifest id
  and uploaded bytes from `--json` output (reuse the `Snapshot` shape) for the
  report.
- **Elevation:** reuse `build_kopia_command` so Linux runs under the `kopia`
  system user when needed, identical to the `kopia_backup` doctor check.

What `<path>` is (the postgres data dir, the configured backup set, etc.) is a
device-config concern that already exists for the EC2/KopiaUI model; reuse the
existing source-path determination rather than inventing one. Flagged as an open
question where it isn't already pinned.

## Backup cadence and the "back up now" signal (transport TBD)

**Canopy is authoritative for *when* a device backs up.** The device holds no
schedule. bestool does not poll a timer; it launches `bestool canopy backup` as
a one-shot process *when Canopy tells it to*. Canopy computes "back up now? /
nothing to do" on each ~1-minute device↔canopy healthcheck tick (the cadence
that already underpins `reachability` and status reporting — see
`alertd`'s status loop and `CanopyClient::post_status`).

**The command-channel transport is not yet specified — note it, don't build it
blind.** Today's status-POST *response* carries no command payload, so there is
no existing channel for Canopy to say "back up now." The canopy-side plan
explicitly defers this (tailnet poll, device poll on the status response, or a
held-open connection) to the repo-alignment pass. For bestool that means:

- The two subcommands above (`backup-credentials`, `backup`) are the stable,
  shippable surface and are *transport-independent*: whatever channel lands,
  it ultimately runs `bestool canopy backup`.
- **Open decision for bestool:** does the daemon (`alertd`) gain a
  backup-trigger task that consumes the signal and spawns `bestool canopy
  backup`, or does an external unit (systemd timer/path, Windows scheduler)
  invoke it? The plan's preferred shape is the existing minute-cadence
  healthcheck carrying the signal, which points at an `alertd` task reading the
  status-response and spawning the backup process. This spec deliberately does
  not pick the transport; it specifies the *command* such that any transport can
  drive it. Implement the transport in a follow-up once the canopy side defines
  the response shape.
- **Operator one-off** and **scheduled** both reduce, device-side, to the same
  thing: Canopy says "back up now," bestool runs `backup`. No device-side
  branching on schedule-vs-manual.

Until the transport exists, `bestool canopy backup` is still fully usable
manually and by any external scheduler, and the dormant-exit-0 behaviour means
it is safe to wire unconditionally into an image ahead of authorization.

## Interfaces / contracts

### Consumed from Canopy (the authoritative contract)

All `ServerDevice`-authenticated (device mTLS), all on `public-server`. Over
tailscale they are under `/public/...`; over public mTLS at the root, matching
how `CanopyClient::get` / `post_event` already split paths.

**`POST /backup-credentials`**
- Request: `{ "purpose": "backup" | "restore" }` (default `"backup"`).
- `200`: the `credential_process` JSON (`Version`/`AccessKeyId`/
  `SecretAccessKey`/`SessionToken`/`Expiration`).
- `412` device not bound to a live server; `409` ungrouped or no backup config;
  `502` STS failed.

**`GET /backup-target`**
- `200`: `{ "storage": "s3", "bucket": str, "prefix": str, "region": str,
  "repo_password": str }`. `prefix` is normally empty (repo at bucket root).
- `412` / `409` as above → **benign dormant** at the driver level (exit 0).

**`POST /backup-report`**
- Request:
  ```json
  {
    "run_id": "<run-uuid>",
    "purpose": "backup" | "restore",
    "outcome": "success" | "failure",
    "error": "...",          // optional, on failure
    "bytes_uploaded": 12345, // optional
    "snapshot_id": "..."     // optional
  }
  ```
- `204` on success.

### Provided by bestool (to Canopy and to the repo's ground truth)

- **The `credential_process` JSON on stdout**, consumed by kopia/the AWS SDK.
- **`run_id` = `backup_runs.id`**: minted client-side, stamped into the snapshot
  tag `canopy-run`, and supplied to `POST /backup-report`. This is the join key
  for `snapshot → run → issuance`. Safe as a client-supplied PK because Canopy
  derives `device_id`/`group_id` from the authenticated `ServerDevice` (not the
  body), and a duplicate id fails its own insert.
- **Snapshot tags** `canopy-device=<device-uuid>` and `canopy-run=<run-uuid>`,
  read by Canopy's read-only inspection job.
- **kopia source host = `<server-id>`**, so sources are `canopy@<server-id>:<path>`
  — the subject-centric attribution Canopy's `backup_repo_snapshots` parses.

### Request/response types (in `crates/canopy/src/client.rs`, re-exported from `lib.rs`)

```rust
#[derive(Serialize)]
pub struct BackupCredentialsRequest { pub purpose: Purpose }

// Typed mirror for tests/validation; the shim relays the raw body, see above.
#[derive(Deserialize)]
pub struct BackupCredentials {
    #[serde(rename = "Version")]        pub version: u8,        // 1
    #[serde(rename = "AccessKeyId")]    pub access_key_id: String,
    #[serde(rename = "SecretAccessKey")] pub secret_access_key: String,
    #[serde(rename = "SessionToken")]   pub session_token: String,
    #[serde(rename = "Expiration")]     pub expiration: jiff::Timestamp,
}

#[derive(Deserialize)]
pub struct BackupTarget {
    pub storage: String,        // "s3"
    pub bucket: String,
    #[serde(default)] pub prefix: String,
    pub region: String,
    pub repo_password: String,  // wrap in Redacted for in-memory handling
}

#[derive(Serialize)]
pub struct BackupReport<'a> {
    pub run_id: &'a str,
    pub purpose: Purpose,
    pub outcome: Outcome,       // Success | Failure, serde lowercase
    #[serde(skip_serializing_if = "Option::is_none")] pub error: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")] pub bytes_uploaded: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")] pub snapshot_id: Option<&'a str>,
}
```

Wrap `repo_password` (and the credential secrets if held in memory at all) in
the existing `bestool_canopy::Redacted<T>` so debug logging can't leak them.
Reuse `jiff::Timestamp` for `Expiration` (already a workspace dep, used by
`NewEvent`). `Purpose` is shared with the action layer.

Add `CanopyClient` methods mirroring `post_event`/`get`:
`backup_credentials(&self, base_url, purpose) -> Result<reqwest::Response>` (the
shim wants the raw body to relay; consider returning the bytes + status),
`backup_target(&self, base_url, purpose) -> Result<TargetOutcome>` where
`TargetOutcome` distinguishes `Ready(BackupTarget)` from `Dormant` (the
412/409 case) so the driver branches cleanly, and `backup_report(&self,
base_url, &BackupReport) -> Result<()>`.

## Testing approach (per repo conventions)

The repo's `AGENTS.md` requires tests with feature work, prefers small deps over
reinvention, and runs `cargo clippy`/`cargo fmt` before commit. DB tests use
`DATABASE_URL=postgresql://localhost/tamanu_meta` — not relevant here (bestool
device side has no DB). Patterns to follow:

- **Unit tests** in each module (`#[cfg(test)] mod tests`), like `register.rs`
  and `kopia/lib.rs`:
  - `Purpose` / request bodies serialize to the exact wire strings
    (`"backup"`/`"restore"`, lowercase outcome), mirroring `severity_serialises_lowercase`.
  - `BackupTarget` / `BackupCredentials` deserialize from representative JSON;
    optional fields omitted in `BackupReport` (mirror `new_event_omits_optional_fields`).
  - The shim's "relay verbatim" property: given a sample 200 body, stdout equals
    the input bytes; given a non-2xx, stdout is empty and exit is non-zero.
  - Run-uuid is minted once and the *same* value lands in both the `canopy-run`
    tag argv and the report body.
  - kopia argv construction: bucket/prefix/region/host-override/tags/credential-process
    are assembled correctly for both purposes (assert on the built `Command`'s
    args, as `kopia/lib.rs` and the doctor check tests do — don't actually run
    kopia).
- **Contract tests** in `crates/bestool/src/canopy_contract.rs` — extend the
  existing `#[ignore]`d live-spec suite (run by the dedicated CI job
  `cargo test -p bestool --lib canopy_contract -- --ignored`). Add:
  - `assert_operation_exists` for `/backup-credentials` (post), `/backup-target`
    (get), `/backup-report` (post).
  - `BackupCredentialsRequest` / `BackupReport` instances validate against the
    spec request schemas (`request_schema(...)`), with a negative case proving
    non-vacuous validation (e.g. an invalid `purpose`/`outcome`).
  - Spec-valid response samples for `/backup-target` and `/backup-credentials`
    decode into `BackupTarget` / `BackupCredentials`.
  This is the mechanism that catches drift against live canopy, and is the right
  home for "does Canopy actually serve what bestool calls."
- **No e2e/playwright** (that's the canopy private-web's concern). bestool has no
  such harness; don't add one.
- Do not actually exec kopia or hit the network in the default `cargo test`
  path; gate anything live behind `#[ignore]` like the contract suite.

## Open questions / decisions to make

1. **Command-channel transport (deferred upstream).** How "back up now" reaches
   the device is unspecified canopy-side. bestool decision: an `alertd` task
   consuming the status-response signal and spawning `bestool canopy backup`, vs.
   an external scheduler invoking the subcommand. Build the subcommands now;
   defer the trigger until canopy defines the status-response command shape.
2. **kopia S3 + `credential_process` wiring.** Confirm against the installed
   kopia whether it consumes AWS creds via the SDK chain (render an
   `AWS_CONFIG_FILE`/`~/.aws/config` with `credential_process = bestool canopy
   backup-credentials`) or via a kopia-native flag. The canopy plan's H-note
   also asks to **verify kopia writes/maintains fine against a default-retention
   (Object-Lock) bucket without client-side `PutObjectRetention`** — device
   creds will lack it. If kopia insists on setting retention, that's a fallback
   the canopy side handles, but bestool must surface the resulting `AccessDenied`
   clearly rather than as opaque kopia failure.
3. **`--override-hostname` flag name/availability** on the installed kopia
   version for setting source host = server id. Verify; the source-encodes-server
   attribution depends on it.
4. **What `restore` does end-to-end.** This spec covers connect-read-only +
   report. Whether `bestool canopy backup --purpose restore` also performs a
   restore-to-disk (and where), or is purely a verification/connect, needs a
   decision — likely a separate `--target <path>` and operator-driven, not part
   of the unattended every-run path.
5. **Backup source path(s).** Reuse the existing device backup-set / postgres
   data-dir determination (the `kopia_backup` doctor check matches
   `*postgresql*` paths) rather than inventing a new config knob. Confirm the
   source of truth for "what to snapshot" on a canopy-managed host.
6. **Concurrency / overlap.** If a backup is still running when the next "back up
   now" arrives, the driver should no-op or refuse rather than start a second
   kopia run (a per-run lockfile, or kopia's own source lock). Decide where the
   guard lives.
7. **Reporting a report-POST failure.** Settled here as: log + exit non-zero, no
   infinite retry (signal-2 inspection backstops). Confirm this matches the
   operator/service expectation (e.g. whether the launching unit treats non-zero
   as "restart and re-backup," which would be wrong — the backup already
   happened).
8. **Repo password handling.** It arrives on `/backup-target` and is passed to
   kopia via `KOPIA_PASSWORD`. Confirm it must never be written to the device's
   persistent kopia config (the every-run-fetch model implies in-memory only);
   if kopia's `repository connect` persists a password hash to its config file,
   decide whether that's acceptable or whether to use a transient config path.
```
---

## Backup types addendum

Per the Canopy plan's "Backup types": bestool **registers the backup types
its server can do**, and owns the "how".

- **A backup-type registry + per-type handlers (the client-side "how").**
  `tamanu-postgres` is the first: quiesce/checkpoint Postgres into a
  consistent state, take a btrfs snapshot, kopia-snapshot the snapshot
  mount, clean up. The procedure is opaque to Canopy. More types later.
- **Register on startup/registration:** `POST /backup-capabilities` with
  the types this server supports.
- **`bestool canopy backup`** runs a *specific type* when Canopy's "back up
  now" signal names it; `backup-credentials` and `backup-report` carry the
  `type`; the run-uuid still = `backup_runs.id`.
- **Per-type repo identity:** source `canopy@<server-id>` with the type in
  the path + a `canopy-type=<type>` tag, so `(server, type)` is one kopia
  source.
- Canopy decides *when* and *which type*; bestool decides *how*.

---

## Implementation-frozen contract (from canopy, 2026-06-16)

Canopy's public-server endpoints are now implemented (canopy PR #224). The
exact request/response shapes — which **supersede the type-less shapes in
"Consumed from Canopy" above** (that section predates the backup-types work) —
are below. The key change: backups are keyed **`(server, type)`**, so `type`
is a required field on `/backup-credentials` and `/backup-report`, and
`/backup-capabilities` is the registration endpoint.

**`POST /backup-capabilities`** — register what this server can back up:
```json
{ "types": ["tamanu-postgres", "..."] }
```
`204`. Canopy upserts `server_backup_capabilities`, seeding each new type's
`enabled` from its `backup_type_defaults.auto_enable`.

**`POST /backup-credentials`**:
```json
{ "type": "tamanu-postgres", "purpose": "backup" }
```
`type` is **required**; `purpose` defaults to `"backup"` (the other value is
`"restore"`). `200` returns the `credential_process` JSON
(`Version`/`AccessKeyId`/`SecretAccessKey`/`SessionToken`/`Expiration`).
`412`/`409`/`502` as before. (`409` also if the `(server, type)` capability
isn't enabled.)

**`GET /backup-target`** — unchanged: `200` →
`{ "storage": "s3", "bucket", "prefix", "region", "repo_password" }`.

**`POST /backup-report`**:
```json
{
  "run_id": "<run-uuid>",
  "type": "tamanu-postgres",
  "purpose": "backup",
  "outcome": "success",
  "error": "...",          // optional, on failure
  "bytes_uploaded": 12345, // optional
  "snapshot_id": "..."     // optional
}
```
`204`. `type` is **required**. A duplicate `run_id` → `409` (the endpoint may
treat it as idempotent; bestool should not rely on re-reporting).

Wire enum values are lowercase: `purpose` ∈ `backup|restore`, `outcome` ∈
`success|failure`. The repo tags bestool must set are unchanged
(`canopy-device`, `canopy-run` = `backup_runs.id`, `canopy-type`), source host
= `<server-id>`.

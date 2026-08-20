---
id: BAK
---

# Canopy backups

bestool is the device-side producer for Canopy's backup control plane.
It advertises which backups a server can take, and when prompted, fetches short-lived per-group object-store credentials and the repository target from Canopy, drives kopia to take a backup, and reports the outcome.
Canopy owns scheduling, retention, maintenance, inspection, and alerting; the device holds no long-lived bucket credentials, never deletes from the repository, and never caches the bucket — the target and credentials are re-derived from Canopy on every run, so a server-side configuration change propagates without per-host action.

## Backup definitions

A backup is configured by a TOML definition file in the backups directory — `/etc/bestool/backups/*.toml` on Unix, a per-platform data directory on Windows — one definition per file (so configuration management can drop in a single file per backup).
A definition carries a `type` (the Canopy-facing label), optional `[tags]` (extra kopia tags), optional ordered `[[pre]]` and `[[post]]` command hooks, and exactly one method table — `[simple]`, `[postgresql]` or `[tamanu_secret_key]` — selecting a built-in method.
A definition with no method table, or with more than one, is a load error.
The `type` is the only identity that matters to Canopy; the filename is informational.

Backups are generic: a definition names a method and a target, and `type` is just a label.
A `tamanu-postgres` backup is a definition that selects the `postgresql` method; there is nothing Tamanu-specific in the machinery.

### Common fields

```toml
type = "tamanu-postgres"          # required — the Canopy backup-type label

[tags]                            # optional — extra kopia tags (string to string)
component = "database"

[[pre]]                           # optional, ordered — run before the snapshot
command = ["/usr/bin/systemctl", "stop", "example"]

[[post]]                          # optional, ordered — run after cleanup
command = ["/usr/bin/systemctl", "start", "example"]
```

A hook is a table with a `command` array, run argv-style (no shell).

### Method tables

There must be exactly one method table.

#### Simple

```toml
[simple]                          # snapshot a path as-is
path = "/var/lib/example"         # required
```

#### PostgreSQL

```toml
[postgresql]                      # crash-consistent postgres cluster snapshot
cluster = "main"                  # required — the cluster to resolve

data_dir = "/var/lib/postgresql/16/main"  # optional — override the resolved data directory
version = "16"                    # optional — override the resolved major version
port = 5432                       # optional — override the port used to issue CHECKPOINT
socket = "/var/run/postgresql"    # optional — override the unix socket directory
```

#### Tamanu secret key

```toml
[tamanu_secret_key]               # capture the key that decrypts local_system_secrets

path = "/etc/tamanu/tamanu.key"   # optional — override the resolved location
package = "central-server"        # optional — which server's config names the key
root = "/opt/tamanu"              # optional — override the discovered install root
```

## Methods

The `simple` method hands kopia a configured path verbatim; it contributes no extra tags and needs no preparation or cleanup.

The `postgresql` method takes a crash-consistent physical copy of a postgres cluster, described under "The postgresql method" below.

The `tamanu_secret_key` method captures the key that decrypts `local_system_secrets`, described under "The tamanu_secret_key method" below.

A method exposes a `prepare` step that produces the path kopia snapshots (plus any method-supplied tags) and a `cleanup` step that releases whatever `prepare` set up; the driver runs the definition's `pre` hooks before `prepare` and its `post` hooks after `cleanup`, and `cleanup`/`post` always run even when the snapshot fails.

## The control-plane contract

The device authenticates to Canopy with the identity established at enrolment — the tailscale path where available, otherwise the device mTLS certificate.
A registration that holds no device key still runs backups and restores over the tailscale path; only a host that has neither a device key nor a reachable tailnet fails, and it fails saying so.
Four endpoints back the system:

- **Register capabilities** — the device posts the set of backup types it can run.
  Canopy records them; a newly-seen type comes up enabled or disabled per Canopy's defaults.
- **Issue credentials** — given a type and a purpose (`backup` or `restore`), Canopy returns short-lived object-store credentials.
  `backup` grants write-without-delete; `restore` is downscoped read-only.
- **Fetch target** — returns the repository target: storage kind, bucket, prefix (normally empty), region, and the repository password.
- **Report a run** — the device posts the run's outcome (success or failure) with the client-minted run id, the type, the purpose, and, on success, the snapshot id and bytes uploaded where known; it also reports the object-storage traffic the run moved and, when the run has one, the moment it froze the data it backed up.
- **Report progress** — while a run is in flight, the device may post cumulative counters describing how far it has got, so Canopy shows a run advancing rather than a figureless in-progress row.

When the device is not yet authorised for backups — not bound to a live server, ungrouped, or the type isn't enabled — the credentials and target endpoints report a benign "not yet authorised" state rather than an error.
Progress reporting has no such gate: a run already under way is described whatever the group's configuration, since refusing progress would blind Canopy precisely when something is misconfigured.

## Taking a backup

A backup run goes through one driver, whether Canopy triggers it through the daemon or an operator runs `bestool canopy backup --type <type>`.
A run:

1. mints a run id (which becomes the report's run id and the `canopy-run` snapshot tag) and resolves the definition for the type, failing fast without touching the network if no definition exists;
2. takes an exclusive per-type lock for the whole run, so a second run for the same type — a re-emitted request, or a manual run racing the daemon — no-ops rather than starting a concurrent kopia.
   The lock lives in a runtime directory and is released by the OS if the process dies;
3. fetches the target.
   A "not yet authorised" response is treated as idle: the run logs that there's nothing to do and exits successfully without reporting.
   This lets a server image ship backup wiring unconditionally and simply wait until an operator authorises the group;
4. starts a loopback re-signing proxy for kopia (below) and connects kopia to the repository through it, reconnecting if the target changed so a server-side bucket change is picked up;
5. runs the `pre` hooks, prepares the method's source, applies an ignore policy for any method-supplied transient files, and takes the kopia snapshot;
6. cleans up and runs the `post` hooks.
   A run asked to hold retains its capture at this point instead of releasing it, as described in [HOLD](held-captures.md);
7. reports the outcome.
   Any run that started kopia reports (success or failure); a run that exited idle at step 3 reports nothing.
   A failed report is logged and surfaced as a non-zero exit, but is not retried — Canopy's repository inspection is the backstop for a lost report.

## The moment the data froze

A run reports the instant it froze the data it backs up — the point in time the backup represents — which is distinct from when the upload finished and from when Canopy received the report.
For a large backup these moments are hours apart, so a run that reports only its finish time misdates its own data.

The freeze moment is the capture's own: where the method takes a point-in-time volume snapshot below kopia, it is the instant that snapshot was taken, which is not recoverable from the repository afterwards and so only the device can report it.
A capture with no distinct freeze instant reports none rather than an approximation: a streamed base backup represents an interval rather than a point, and a plain path snapshotted live has no consistency point at all.
The moment is reported as early as it is known — before any transfer begins — so it is carried on the first progress report where there is one, and on the completion report regardless.
Canopy records it once per run and the first value stands, so sending it early rather than only at the end is what keeps a long run's data correctly dated.

## Reporting progress

While a run is in flight the device posts progress as often as it chooses, at a cadence of its own picking within the rate Canopy accepts, clamped so a misconfiguration cannot post in a tight loop.
Progress is best-effort telemetry: a refused or failed progress post — whatever the cause — is logged and the run carries on, never aborted, and a run that reports no progress at all is backed up and reported exactly as one that does.

Each progress report carries the run id, the type and purpose, and two independent families of cumulative-since-run-start counters:

- the backup engine's own work — source bytes read, bytes processed, bytes uploaded, bytes already present, the total the run expects, files done and expected, and errors hit and ignored — together with what the run is working on at that moment;
- the object-storage traffic the run has moved — the same raw and payload, sent and received figures the completed run reports — as tallied so far.

All counters are totals from the start of the run rather than per-interval deltas, so a lost or repeated report costs only resolution and never corrupts a total, and Canopy can take a run's final figure from the last progress report where the completion report omits it.
The device omits any counter it does not measure, so that an absent figure reads as unknown rather than as a stalled zero.
The engine's figures are as precise as the engine exposes mid-run — a coarser resolution than the exact total on the completion report — and the run's verbatim engine status line is included as opaque detail Canopy stores and shows without interpreting.

Both a backup and a restore report progress this way.
A restore reports the object-storage traffic it has moved — for a restore, dominated by the bytes downloaded — which answers whether it is progressing; a restore has no freeze moment and need not report engine counters.

The repository password is a real secret and is kept reasonably protected from leakage — out of the process argument list and out of any persisted configuration, so it can't be read from a process listing or left on the device.
The S3 credentials kopia is given need no such protection: they are dummy values, the real credentials living only in the re-signing proxy ([S3P](s3-sigv4-proxy.md)).
kopia runs against a transient per-run config, so the device never holds the bucket either.

## Credentials

kopia binds its S3 credentials once at start-up and has no mid-run refresh, while Canopy's assumed-role credentials are short-lived — so a long operation would otherwise outlive them.
The driver bridges this with a loopback re-signing proxy ([S3P](s3-sigv4-proxy.md)): kopia is pointed at the proxy with meaningless dummy keys, and the proxy re-signs each request with live credentials fetched from Canopy, refreshed as they near expiry.
A long run is bounded by how long Canopy stays reachable to reissue credentials, not by a single issuance.
Environment variables that would otherwise let the host's ambient credentials shadow the dummy keys are scrubbed from kopia's environment.

## Repository identity and tags

kopia's snapshot source host is set to the server id, so a backup's source is attributed to the backup subject and survives device replacement with continuous history; the username is fixed.
The source path is stable across runs for a given backup type, so kopia's snapshot history, deduplication, and retention attribute to one source.

Every snapshot is tagged with the device id, the run id, and the backup type, plus any tags the definition or the method contribute; the canopy-owned tags take precedence so a definition cannot override them.

## Local cache

kopia keeps a local cache of repository data next to its configuration, and a device is a working server whose disk is sized for the data it serves, not for a backup tool's scratch space.
So the cache is bounded on every connection to the repository, rather than left at whatever the tool defaults to.
The bound is a share of the volume the cache lives on, so the same rule suits a small appliance and a large server: a connection that takes backups may use 5% of that volume, and one that restores may use 20%.
A restore is an attended operation whose whole job is reading data back, so it is worth the disk; a backup runs behind a live workload and is not.
Whatever that share works out to, the cache never takes more than half the space free on the volume, so a backup cannot be what fills a disk that is already nearly full.
A host whose share would be too small for the cache to be worth keeping gets a modest fixed budget instead, unless the free-space limit is what made it small; a host that cannot measure its volume at all gets that fixed budget too.
The share can be overridden with an absolute size per host.

The budget is divided between the cached copies of backed-up file data and of repository metadata, and how it divides also depends on what the connection is for.
A device taking backups never reads file data back out of the repository — restores are an operator action and the repository's upkeep is Canopy's — so a backup connection gives most of its budget to metadata, which is what lets an unchanged file be recognised from the previous snapshot without being read and hashed again.
A restore connection reverses the split.

The driver applies the bound whenever it connects, so a host connected under an earlier rule is corrected by its next run without being touched.
The bound is on the caches whose size can be set, so a device's total cache use is its budget plus the smaller caches the tool keeps unbounded; the budget is not the whole footprint.

## Registration and triggering by the daemon

When run under the bestool-alertd daemon, the device registers its capabilities — the types of every definition in the backups directory — with Canopy at startup, again on reload, and periodically as a safety net.
A reload is triggered by the daemon's reload signal or its control endpoint, and a change to the backups directory is picked up by watching it, so dropping in a new definition is registered without a restart.

Canopy decides when a server backs up.
On each device-to-Canopy healthcheck tick, Canopy's response names the backup types the server should run right now (the union of operator one-offs and schedule-due types; empty means nothing to do).
The daemon runs each named type's driver in-process, skipping any type whose previous run is still going.
Reporting a run clears the corresponding one-off, so the heartbeat stops re-emitting it.

`bestool canopy backup` prefers the running daemon: it asks the daemon to run the named type and streams the run's progress and outcome back, so a manual backup takes the same in-process path, environment, and per-type lock as a Canopy-triggered one, and a manual run cannot run concurrently with a scheduled one. When no daemon is reachable, or `--no-daemon` is given, the command runs the backup itself. The run is identical either way; only the process hosting it differs.

## The postgresql method

The method produces an atomic, crash-consistent copy of the cluster and never writes a `backup_label`, so a restore is plain crash recovery — the cluster replays its WAL to a consistent state.
This is what keeps restores clean: it avoids the forced WAL reset and full reindex that a partial backup label or a non-atomic copy provoke downstream.
An explicit CHECKPOINT is issued just before the capture to bound how much WAL the restore replays; it is an optimisation, not a correctness requirement.

The method is generic postgres, driven by its configuration (a cluster name, with optional data-directory, version, port, and socket overrides) rather than by any application's configuration.
It resolves the cluster's data directory and the volumes the cluster occupies, then captures by the cheapest consistent means the storage offers: where the underlying volume can take a cheap, point-in-time read-only snapshot, it snapshots the volume; otherwise it streams a `pg_basebackup` base backup, which bundles the WAL and the backup-end record so it too restores by clean crash recovery.

A volume snapshot necessarily freezes the whole volume the data directory lives on — it is taken at the volume or block level, not of a bare subdirectory — but kopia only backs up the cluster's subdirectory within the frozen, read-only mount, exposed at the stable source path.
Transient files (the postmaster lock, logs, the stats temp directory) are ignored; the WAL, transaction-status, control, global, and tablespace data never are.

If a snapshot cannot be taken — the volume's snapshot mechanism is unavailable, insufficient privilege, or a multi-volume layout that cannot be frozen atomically — the method falls back to `pg_basebackup` rather than fail.
This is a safe degradation to a correct, if heavier, base backup; it never falls back to reading the live data directory.
A capture never silently degrades to an unsafe copy.

Before creating a capture the method sweeps leftovers from a previously crashed run (a hard reboot skips cleanup), so orphaned snapshots and mounts do not accumulate.
Backups run with the privilege the capture needs.

### What the cluster and host must provide

The method connects as the postgres superuser over the local unix socket — peer authentication, no password.
So the host must let bestool become that superuser: it runs as root (or equivalent), and the cluster keeps local socket peer auth for the superuser (the default).
No dedicated backup role, password, or TCP access is needed; set the socket directory or port in the def only when they aren't the defaults.

The snapshot capture needs the cluster's data directory — with its WAL and any tablespaces — on a single volume that can take a cheap, consistent point-in-time snapshot.
Data spread across volumes that can't be frozen together falls back to the base backup.
`full_page_writes` must be left on (the default), since recovery from a volume snapshot relies on it.

The base-backup fallback streams over the replication protocol, so the cluster must allow replication: `wal_level` at least `replica` and `max_wal_senders` above zero (both defaults), and a local replication entry for the superuser in the host-based auth configuration.
The superuser connection already carries the replication privilege.

## The tamanu_secret_key method

The key at `crypto.keyFile` encrypts every value in `local_system_secrets`: the settings PSK (and so every secret setting), the device key, and a facility's sync password.
A database restored onto a host holding a different key reads none of them, so the key has to be captured with the database it belongs to.

Where the key lives is a property of the install, not of the definition, so the method resolves it rather than having an operator name a path per host.
A bare-metal or Windows install points `crypto.keyFile` at a file, relative paths resolving against the server package directory as they do for the server itself.
A containerised install holds the key as a podman secret, mounted into the containers at the `/run/secrets` path its `crypto.keyFile` names; the path's basename names the secret, and the value is read and written through podman.
Only the one secret is touched: the host's secret store also holds values that belong to the host rather than to the database, and none of those should travel with a backup.
The file has to exist to be chosen, so a `crypto.keyFile` that names neither an existing file nor a `/run/secrets` path is an error rather than a silent capture of the wrong thing.

What lands in the repository is the key value itself, one file under a fixed name, whatever shape the host held it in.
The value is all the database needs, which is what lets a capture taken from one shape restore onto an install of the other.

The capture is a copy taken before kopia reads it, which is what makes it a point in time; a key is a few hundred bytes, so copying is cheaper than holding a consistent view for the length of a snapshot.
It is not offered as a rollback point: re-capturing a key costs nothing, so there is nothing a hold would buy.

A restore writes the value back in whatever shape this host keeps its key, converting freely between shapes: onto the key file path by an atomic rename, or into the podman secret through podman, picked up when the containers next start.
Either way the key it displaces is kept beside it, as `<name>.old` next to a key file or as a `.old`-suffixed secret in podman.

## Restore

`bestool canopy restore <type> <id>` is the operator-facing restore.
The backup type selects the definition, method, and credential scope; the snapshot to restore is named explicitly by id.
It resolves the definition, fetches restore-purpose (read-only) credentials, connects to the repository, and selects the snapshot whose id matches.
A restore can equally take its source from a capture held on the device, described in [HOLD](held-captures.md), in which case nothing is fetched or downloaded and the method's restore is otherwise identical.
Selection is by id across the whole repository — not scoped to the server issuing the restore — so a replacement host can restore a backup taken by the server it succeeds.
It restores the snapshot into a staging area on the same filesystem as the target so the final move is atomic, then hands off to the method.

The `postgresql` method's restore is a full automated swap: it stops the cluster, moves the existing data directory aside (kept, not deleted), moves the restored tree into place with the right ownership and permissions, starts the cluster via plain crash recovery, and verifies it accepts connections.
A WAL reset is only attempted as a logged last resort if the cluster will not start.
The `simple` method's restore lays the files back at its path or a given target.
The `tamanu_secret_key` method's restore lays the key back where this host keeps it, described below.

Restore refuses to overwrite existing data by default.
To proceed an operator passes an explicit confirmation flag (for non-interactive use) or answers an interactive double confirmation; with neither, over occupied data, it refuses.
Migrations, configuration sync, and version upgrades are left to the operator.

Off-host restore verification is Canopy's concern, not this command's; this command's job is to produce clean backups and to restore them on demand.

## Ad-hoc repository access

`bestool canopy kopia --type <type> [--purpose backup|restore] -- <args>` runs an arbitrary kopia command against the repository, for inspection and maintenance without hand-wiring credentials.
It fetches credentials of the requested purpose (defaulting to read-only restore), connects through the same loopback proxy, and runs the given kopia arguments with the operator's own input, output, and exit status.
It grants no access the purpose's credentials don't already carry: a `restore`-purpose command cannot write, and no purpose can delete.

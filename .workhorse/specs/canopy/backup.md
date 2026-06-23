---
id: BAK
---

# Canopy backups

bestool is the device-side producer for Canopy's backup control plane. It advertises which backups a server can take, and when prompted, fetches short-lived per-group object-store credentials and the repository target from Canopy, drives kopia to take a backup, and reports the outcome. Canopy owns scheduling, retention, maintenance, inspection, and alerting; the device holds no long-lived bucket credentials, never deletes from the repository, and never caches the bucket — the target and credentials are re-derived from Canopy on every run, so a server-side configuration change propagates without per-host action.

## Backup definitions

A backup is configured by a definition file in the backups directory — `/etc/bestool/backups/*.toml` on Unix, a per-platform data directory on Windows — one definition per file (so configuration management can drop in a single file per backup). A definition carries a `type` (the Canopy-facing label), optional `[tags]` (extra kopia tags), optional ordered `[[pre]]` and `[[post]]` command hooks, and exactly one method table — `[simple]` or `[postgresql]` — selecting a built-in method. A definition with no method table, or with more than one, is a load error. The `type` is the only identity that matters to Canopy; the filename is informational.

Backups are generic: a definition names a method and a target, and `type` is just a label. A `tamanu-postgres` backup is a definition that selects the `postgresql` method; there is nothing Tamanu-specific in the machinery.

## Methods

The `simple` method hands kopia a configured path verbatim; it contributes no extra tags and needs no preparation or cleanup.

The `postgresql` method takes a crash-consistent physical copy of a postgres cluster, described under "The postgresql method" below.

A method exposes a `prepare` step that produces the path kopia snapshots (plus any method-supplied tags) and a `cleanup` step that releases whatever `prepare` set up; the driver runs the definition's `pre` hooks before `prepare` and its `post` hooks after `cleanup`, and `cleanup`/`post` always run even when the snapshot fails.

## The control-plane contract

The device authenticates to Canopy with the identity established at enrolment — the tailscale path where available, otherwise the device mTLS certificate. Four endpoints back the system:

- **Register capabilities** — the device posts the set of backup types it can run. Canopy records them; a newly-seen type comes up enabled or disabled per Canopy's defaults.
- **Issue credentials** — given a type and a purpose (`backup` or `restore`), Canopy returns short-lived object-store credentials. `backup` grants write-without-delete; `restore` is downscoped read-only.
- **Fetch target** — returns the repository target: storage kind, bucket, prefix (normally empty), region, and the repository password.
- **Report a run** — the device posts the run's outcome (success or failure) with the client-minted run id, the type, the purpose, and, on success, the snapshot id and bytes uploaded where known.

When the device is not yet authorised for backups — not bound to a live server, ungrouped, or the type isn't enabled — the credentials and target endpoints report a benign "not yet authorised" state rather than an error.

## Taking a backup

`bestool canopy backup --type <type>` drives one run, and is also what the daemon invokes in-process when Canopy asks for a backup. A run:

1. mints a run id (which becomes the report's run id and the `canopy-run` snapshot tag) and resolves the definition for the type, failing fast without touching the network if no definition exists;
2. takes an exclusive per-type lock for the whole run, so a second run for the same type — a re-emitted request, or a manual run racing the daemon — no-ops rather than starting a concurrent kopia. The lock lives in a runtime directory and is released by the OS if the process dies;
3. fetches the target. A "not yet authorised" response is treated as idle: the run logs that there's nothing to do and exits successfully without reporting. This lets a server image ship backup wiring unconditionally and simply wait until an operator authorises the group;
4. starts a loopback credentials endpoint for kopia (below) and connects kopia to the repository, reconnecting if the target changed so a server-side bucket change is picked up;
5. runs the `pre` hooks, prepares the method's source, applies an ignore policy for any method-supplied transient files, and takes the kopia snapshot;
6. cleans up and runs the `post` hooks;
7. reports the outcome. Any run that started kopia reports (success or failure); a run that exited idle at step 3 reports nothing. A failed report is logged and surfaced as a non-zero exit, but is not retried — Canopy's repository inspection is the backstop for a lost report.

The repository password reaches kopia by environment and the bucket details by command line; neither is written to persistent device configuration (kopia runs against a transient per-run config), so the device never holds the bucket.

## The credentials endpoint

kopia's object-store backend obtains credentials from an ECS-style container-credentials endpoint and self-refreshes by re-polling it; it cannot consume a credential-process shim or a static credentials file. So the driver serves a loopback HTTP endpoint, bound to a loopback literal, and points kopia at it by environment. Each run leases a random bearer token; a request carrying that token receives the cached credentials, an unknown or absent token is refused, and the token is deregistered when the run ends so a leaked token stops working.

The endpoint fetches credentials from Canopy on first use and again as they approach expiry, translating Canopy's credential-process-shaped response into the container-credentials shape kopia expects. Because each issuance is short-lived, a long run simply re-fetches; Canopy must stay reachable for the whole run, not just the start. Environment variables that would otherwise let the host's ambient credentials shadow the endpoint are scrubbed from kopia's environment.

## Repository identity and tags

kopia's snapshot source host is set to the server id, so a backup's source is attributed to the backup subject and survives device replacement with continuous history; the username is fixed. The source path is stable across runs for a given backup type, so kopia's snapshot history, deduplication, and retention attribute to one source.

Every snapshot is tagged with the device id, the run id, and the backup type, plus any tags the definition or the method contribute; the canopy-owned tags take precedence so a definition cannot override them.

## Registration and triggering by the daemon

When run under the bestool-alertd daemon, the device registers its capabilities — the types of every definition in the backups directory — with Canopy at startup, again on reload, and periodically as a safety net. A reload is triggered by the daemon's reload signal or its control endpoint, and a change to the backups directory is picked up by watching it, so dropping in a new definition is registered without a restart.

Canopy decides when a server backs up. On each device-to-Canopy healthcheck tick, Canopy's response names the backup types the server should run right now (the union of operator one-offs and schedule-due types; empty means nothing to do). The daemon runs each named type's driver in-process, skipping any type whose previous run is still going. Reporting a run clears the corresponding one-off, so the heartbeat stops re-emitting it.

A standalone `bestool canopy backup` run works without the daemon, for manual use or an external scheduler; it serves its own ephemeral credentials endpoint for the run.

## The postgresql method

The method produces an atomic, crash-consistent copy of the cluster and never writes a `backup_label`, so a restore is plain crash recovery — the cluster replays its WAL to a consistent state. This is what keeps restores clean: it avoids the forced WAL reset and full reindex that a partial backup label or a non-atomic copy provoke downstream. An explicit CHECKPOINT is issued just before the capture to bound how much WAL the restore replays; it is an optimisation, not a correctness requirement.

The method is generic postgres, driven by its configuration (a cluster name, with optional data-directory, version, port, and socket overrides) rather than by any application's configuration. It resolves the cluster's data directory, enumerates the volumes the cluster occupies, and picks a capture backend from the storage:

- a **btrfs** filesystem takes a read-only subvolume snapshot;
- a backing **thin LVM** volume takes a thin snapshot;
- **Windows** takes a VSS shadow copy;
- anything else (a plain partition, a thick LVM volume) streams a **`pg_basebackup`** base backup, which bundles the WAL and the backup-end record so it too restores by clean crash recovery.

The snapshot backends necessarily freeze the whole subvolume or volume the data directory lives on — a snapshot is taken at the subvolume or block level, not of a bare subdirectory — but kopia only backs up the cluster's subdirectory within the frozen, read-only mount, exposed at the stable source path. Transient files (the postmaster lock, logs, the stats temp directory) are ignored; the WAL, transaction-status, control, global, and tablespace data never are.

If a snapshot backend cannot capture — VSS unavailable, insufficient privilege, or a multi-volume layout that cannot be frozen atomically — the method falls back to `pg_basebackup` rather than fail. This is a safe degradation to a correct, if heavier, base backup; it never falls back to reading the live data directory. A backend never silently degrades to an unsafe copy.

Before creating a capture the method sweeps leftovers from a previously crashed run (a hard reboot skips cleanup), so orphaned snapshots and mounts do not accumulate. Backups run with the privilege the capture needs, and the postgres tools are located beside the data directory where they are not on the path.

## Restore

`bestool canopy restore --type <type>` is the operator-facing restore. It resolves the definition, fetches restore-purpose (read-only) credentials, connects to the repository, and selects a snapshot — the latest, or one named by id — filtered to this server and this backup type. It restores the snapshot into a staging area on the same filesystem as the target so the final move is atomic, then hands off to the method.

The `postgresql` method's restore is a full automated swap: it stops the cluster, moves the existing data directory aside (kept, not deleted), moves the restored tree into place with the right ownership and permissions, starts the cluster via plain crash recovery, and verifies it accepts connections. A WAL reset is only attempted as a logged last resort if the cluster will not start. The `simple` method's restore lays the files back at its path or a given target.

Restore refuses to overwrite existing data by default. To proceed an operator passes an explicit confirmation flag (for non-interactive use) or answers an interactive double confirmation; with neither, over occupied data, it refuses. Migrations, configuration sync, and version upgrades are left to the operator.

Off-host restore verification is Canopy's concern, not this command's; this command's job is to produce clean backups and to restore them on demand.

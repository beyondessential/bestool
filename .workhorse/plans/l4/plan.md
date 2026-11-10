# Barman-style live postgres backups through Canopy

Exploration card: how Linux hosts could get continuous, point-in-time-recoverable postgres backups using barman's model, how that fits Canopy, whether CNPG's barman backups can be brought under Canopy, and whether bestool can restore a CNPG backup onto a plain Linux install.

## What exists today

### bestool on Linux hosts (BAK)

- The `postgresql` method takes a crash-consistent volume snapshot (btrfs, thin-LVM) or falls back to `pg_basebackup --wal-method=stream`, then kopia uploads the cluster directory.
- No `backup_label` is written; a restore is plain crash recovery.
- kopia deduplicates the upload, but every changed 1 GiB segment file is read and rechunked in full on every run, and the recovery point is the last scheduled run (daily or whatever interval Canopy holds).
- There is no WAL archiving, so no point-in-time recovery and no RPO below the backup interval.

### Canopy server

- One kopia repository per group, one bucket per group (`bes-canopy-backup-<group>-<random>`), Object Lock GOVERNANCE 30 days plus versioning.
- Device credentials: STS AssumeRole with an inline session policy scoped to `bucket/prefix*`. `backup` grants get/put/delete (delete only writes a marker on the locked bucket); `restore` grants get and list.
- Everything is kopia-shaped: the bucket probe recognises only `kopia.repository`; retention is kopia's `keep-*` policy; inspection reads kopia manifests; the restore worklist hands out a kopia snapshot id; staleness is measured per snapshot run.
- Runs are one-shot, keyed by run id, triggered by `backup_now` in the status response. There is no notion of a continuously running job.
- No notion of a Kubernetes-hosted server: every backup resolves from an authenticated device to its machine to its group. Canopy's own fleet spec defers this to the Kubernetes project.

### CNPG at BES (infra repo)

- CNPG with the Barman Cloud Plugin (not the deprecated in-tree `barmanObjectStore`). Postgres 18 by default.
- One bucket per Kubernetes cluster, `bes-k8s-<cluster>-backups`; objects at `s3://<bucket>/<deployment>/<db-cluster>/<serverName>/{base,wals}`.
- IRSA credentials; gzip for both data and WAL; no client-side encryption; retention by recovery window (3 days for Tamanu deployments, 30 for Canopy's own database).
- Daily scheduled base backup plus continuous WAL archiving. Volume snapshots are deliberately not used for backups.
- Every restore today goes through CNPG recovery in a scratch namespace, then `pg_dump | pg_restore`. Nothing restores a CNPG backup outside Kubernetes.
- postgres-restore-operator restores kopia snapshots from Linux hosts into k8s; it does not touch barman.

### barman-cloud format and tools

- Layout under `<prefix>/<serverName>/`:
  - `base/<backup_id>/backup.info` (key=value; written `STARTED`, rewritten `DONE`/`FAILED`), `data.tar[.gz|...]` (split as `data_0001.tar...` past the max archive size), one `<oid>.tar` per tablespace. Tar paths are relative to PGDATA; `backup_label` is injected.
  - `wals/<timeline><log-hi>/<segment>[.gz|...]`, with `.backup` and `.partial` alongside; history files directly under `wals/`.
- CNPG (in-tree and plugin) runs the same Python CLIs, so the layout is identical.
- `barman-cloud-backup` uses the concurrent backup API over libpq, walks the local PGDATA, spools tar parts to TMPDIR and uploads with multipart. No incremental base backups: every base backup is a full upload with no dedup.
- `barman-cloud-backup-delete` applies redundancy or recovery-window retention and deletes WALs no remaining backup needs.
- Credentials: plain boto3 chain; `credential_process` refreshes, static env credentials do not.
- Python 3.12+, boto3, psycopg2; packaged as `barman-cli-cloud` in PGDG. GPL-3.0. No non-Python implementation of the format exists.

### Restoring a CNPG backup on a plain host: known hazards

- Major version must match exactly; every extension and `shared_preload_libraries` entry must be installed; glibc/ICU collation versions should match the CNPG image or text indexes need rebuilding.
- CNPG's config lives inside PGDATA and points at `/controller/...` paths (archive command, TLS files, socket dir), `listen_addresses='*'`, `restart_after_crash=false`, `wal_level=logical`. All of it needs rewriting.
- Ownership is uid 26 in the image; needs `chown` to the host's postgres user.
- If the cluster used a separate WAL volume, `pg_wal` is likely absent from the tar and must be recreated.
- A promoted restore starts a new timeline; it must never archive into the source's path.

## Decisions

### Goals

All three: point-in-time recovery with a short RPO, base backups that stay deduplicated, and one format shared with CNPG wherever it can be.

### Hybrid: kopia base backups, barman-layout WAL archive

Linux hosts keep the existing `postgresql` method (volume snapshot or base backup, uploaded by kopia) as their base backups, and gain continuous WAL archiving written in barman's `wals/` layout beside the kopia repository.

- Base backups keep kopia's dedup, which barman-cloud cannot offer: every barman-cloud base backup is a full upload.
- With WAL covering the gaps, base backups can run less often without widening the RPO, which is the other half of making them lighter.
- The shared format is the WAL archive and the restore tooling, not the base backups. A Linux restore is a kopia base plus barman-layout WAL; a CNPG restore is a barman tar base plus barman-layout WAL; both replay WAL through the same fetcher and the same recovery configuration.
- Premise to verify before building: postgres performs archive recovery correctly from an atomic volume snapshot that carries no `backup_label`, replaying from the snapshot's checkpoint through the archive to a target time. The `pg_basebackup` fallback writes a `backup_label` and is already a valid PITR base.

### Native implementation of the barman layout

bestool reads and writes the barman object layout itself rather than driving the barman-cloud CLIs.

- The CLIs need Python 3.12+, boto3 and psycopg2, which Ubuntu 22.04 hosts don't have, and they bind credentials once per run.
- The surface is small: WAL push is a single PUT of `wals/<dir>/<segment>.gz`, WAL fetch a GET, and a CNPG base backup is `backup.info` plus streamed gzip tars.
- S3 requests are signed with the SigV4 signer bestool already carries for the kopia re-signing proxy, so no AWS SDK is added.
- Format drift is guarded by tests against fixtures produced by real barman-cloud.

### WAL transport: `archive_command` hands off to the daemon

Postgres's `archive_command` invokes bestool with the segment path, which hands it to the alertd daemon over its control socket; the daemon holds Canopy credentials and uploads it.

- A segment counts as archived only once it is offsite, so postgres retains WAL until then and `pg_stat_archiver` tells the truth; alertd alerts on archive failures.
- When the daemon is unreachable, the command falls back to uploading the segment itself with per-invocation credentials, so a stopped daemon delays nothing beyond credential cost.
- The daemon never depends on postgres to start; postgres depends on the daemon only for archiving, which degrades to the fallback rather than blocking.

## Open design questions

- `archive_timeout` value (sets the RPO on quiet servers) and whether bestool sets it or the definition does.
- Where the WAL archive sits in the group bucket next to the kopia repository (kopia's objects are at the bucket root), and how it is keyed per server and cluster.
- WAL retention: devices cannot delete, so Canopy maintenance prunes WAL older than the recovery window, keeping whatever the oldest in-window base backup needs.
- How Canopy represents a continuous archive: staleness of the newest WAL, recoverable window, gaps.
- How CNPG archives get into Canopy at all, given Canopy has no Kubernetes-hosted server and the buckets are per k8s cluster with IRSA.

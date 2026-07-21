# Backup space pre-check + roomier-disk staging

## Problem

The `pg_basebackup` strategy streams a full copy of the cluster into a staging
dir before kopia snapshots it. On Windows that staging lives on the system drive
(`%ProgramData%\bestool\backup-source\…`). On a host whose system drive is tight
but which has a separate data disk (`D:`, `E:`, …) with a `Backup`/`Backups`
folder and plenty of room, the copy can exhaust the system drive and the backup
fails partway with no early warning.

Two asks:
1. Predict how much space a base backup needs and bail early (reported to canopy)
   if there isn't enough.
2. Prefer a roomier disk (a `Backup`/`Backups` folder on another drive) for the
   staging copy when the system drive is too small.

## Decisions (agreed)

- **Disk selection:** auto-detect with a config override; fall back to the system
  drive. Config override always wins (and bails if it doesn't fit).
- **Kopia path may move freely.** Verified: canopy restore selects snapshots by
  snapshot ID only (`select_snapshot` in `actions/canopy/restore.rs` filters on
  `id.starts_with`; `kopia snapshot restore` takes the ID into a fresh staging
  dir). Nothing keys on the source path, so relocating staging never breaks
  restore of older snapshots. No junctions.
- **Size estimate:** `max(SQL sum(pg_database_size) over all databases, on-disk
  walk of the data dir)`.
- **Scope of the gate:** the postgres method only — basebackup staging selection
  + gate, and a lighter VSS free-space pre-check. btrfs/thin-LVM capture in place
  (no staging copy) and are untouched.
- **All failures report via canopy.** Achieved by construction: the checks live
  inside `prepare`, which runs within `backup_after_start`'s reporting envelope,
  so any `Err` becomes a `RunOutcome::Failure` report with the message.

## Design

### Free space + estimate primitives

New module `crates/bestool/src/actions/canopy/backup/postgresql/space.rs`:

- `available(path) -> Option<u64>` — wraps `fs4::available_space` (already a
  dependency), `None` on error (never block a backup because a stat failed).
- `dir_size(path) -> u64` — recursive on-disk size of the data dir (async walk,
  stat-only). Best-effort: unreadable entries are skipped, not fatal.
- `db_size_sql(config) -> Option<u64>` — run
  `psql -X -q -w -tAc "SELECT COALESCE(sum(pg_database_size(oid)),0) FROM pg_database"`
  via the existing `pg_command` / `apply_connection`, parse the integer. `None`
  on any failure.
- `estimate_needed(config, data_dir) -> Option<u64>` — `max` of the two above;
  `None` only if *both* fail (caller then logs and proceeds without gating).
- `required_free(need) -> u64` — `need + max(need/5, 1 GiB)` (20% headroom, floor
  1 GiB). Constants named and documented.

Pure/unit-tested: `required_free`, `estimate_needed` combining `Some`/`None`
inputs, `dir_size` over a tempdir. The live `psql` call and `fs4` on real volumes
stay verified on-host.

### Staging-root selection (basebackup)

New `staging_dir: Option<PathBuf>` on `PostgresqlConfig` (config override). Also
threaded so `stable_source_dir`'s caller can pass a chosen root; the current
hardcoded default becomes the fallback candidate.

`choose_staging_root(backup_type, override, need) -> Result<PathBuf>`:

- If `override` is set: use `<override>/backup-source/<type>`; if its available
  space `< required_free(need)`, bail (respect the explicit choice, don't silently
  ignore it).
- Else build candidates:
  - the system default (`stable_source_dir` as today), and
  - **Windows only:** each fixed drive carrying a top-level `Backup` or `Backups`
    folder (case-insensitive), staging under
    `<drive>\Backup[s]\bestool\backup-source\<type>`.
- Enumerate fixed drives via `sysinfo` (`Disks`, mount points + kind); skip
  removable and network disks. No `unsafe` FFI.
- Pick the candidate whose available space `>= required_free(need)`, preferring a
  detected backup-folder disk with the most free space, else the system default.
- If none fits, bail early naming each candidate and its free space plus the
  estimate + headroom.

On Unix there is no drive-letter/`Backup`-folder convention, so auto-detect is a
no-op: only the system default and the config override apply.

Selection is factored as a pure function over `[(PathBuf, Option<u64> available)]`
+ `need` so the preference/bail logic is unit-tested without touching disks; the
drive enumeration and backup-folder detection are thin, separately-tested shells.

### Wiring into `prepare`

- `postgresql::prepare` computes `need = estimate_needed(...)` once.
- basebackup path: `choose_staging_root` replaces the hardcoded root; the chosen
  root flows into `destination`/staging and is returned as the teardown root and
  the kopia source path. If `need` is `None` (estimation failed), skip the gate
  but still honour the config override / default root.
- The existing prepare cleanup, chown, and orphan guard (PR #712) operate on the
  chosen root unchanged.

### VSS pre-check

In `vss::prepare`, before creating the shadow: require the data volume to have at
least `max(1 GiB, need/10)` free (COW diff space is write-proportional, not
full-DB, so this is a lighter floor). Bail with a clear message otherwise. VSS is
not relocated (the shadow lives on the source volume). If `need` is `None`, fall
back to the 1 GiB floor.

### Reporting

No new reporting code. All bails are ordinary `Err` from within `prepare`, which
`backup_after_start` already turns into a canopy `RunOutcome::Failure` report
carrying the message (`actions/canopy/backup.rs`). Error messages state the
estimate, the headroom, and per-candidate free space so the report is actionable.

## Non-goals

- Junctions / keeping a stable kopia path across disks.
- VSS shadow-storage relocation to another volume.
- Auto-detect on Unix.
- Making headroom/threshold constants configurable (fixed sensible defaults;
  revisit if needed).

## Testing

- Unit: `required_free` math; `estimate_needed` Some/None combinations;
  `choose_staging_root` preference + bail over synthetic `(root, available)` sets;
  backup-folder detection over a fake FS; `dir_size` over a tempdir.
- On-host (Windows) verification: real drive enumeration, a `Backup` folder on a
  second drive actually being chosen, the gate bailing when the system drive is
  full, the VSS pre-check, and the canopy failure report landing.

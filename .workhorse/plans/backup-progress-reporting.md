# Backup progress reporting (bestool side)

Spec: [BAK](../specs/canopy/backup.md) — the device-facing "Reporting progress" and "The moment the data froze" sections.
Canopy side is shipped: `POST /backup-progress` and `ReportArgs.snapshot_taken_at` are live in the OpenAPI document.

## Problem

A run in flight appears in Canopy as a figureless in-progress row: the only evidence it exists is the credential issuance that started it.
bestool already computes the numbers that would answer "is it moving, or wedged" — kopia's progress and the proxy's S3 tallies — but neither reaches Canopy until the run ends.
Separately, a run that freezes its data below kopia (a volume snapshot) leaves no trace of *when* it froze in the repository, so a long run's data is misdated to its report time unless the device reports the freeze instant itself.

## What Canopy accepts

`POST /backup-progress`, same device auth as `/backup-report`. All fields optional except `run_id` and `type`.
Counters are **cumulative from run start**, not per-interval deltas: send totals-so-far every sample.
Omit a counter that isn't measured (absent = unknown; a zero reads as stalled).
`run_id` must be the same uuid used at `/backup-credentials` and `/backup-report`.
A refused or failed post (429/409/412/timeout/5xx) is telemetry — log and carry on, never abort the run.
`snapshot_taken_at` is write-once per run, first value wins; send it as early as known.

## Pieces

### 1. Regenerate `bestool-canopy`

Force a live fetch so the generated schema picks up `ProgressArgs`, the `client.backup_progress()` method, and `snapshot_taken_at` on `ReportArgs`.
No build.rs change: the `date-time` → `jiff::Timestamp` rewrite is format-driven, so both new timestamp fields land as `Option<Timestamp>` automatically.
Update the committed `openapi.snapshot.json` and the `OPENAPI_BLAKE3` digest.

### 2. kopia progress parser — `crates/kopia/src/progress.rs`

A winnow parser over kopia's foreground `--progress` line. Real 0.23.1 format:

```
 | 3 hashing, 0 hashed (65.5 KB), 0 cached (0 B), uploaded 0 B, estimating...
 * 0 hashing, 3 hashed (140 MB), 0 cached (0 B), uploaded 137.5 MB, estimating...
```

and once estimation completes, the tail becomes `estimated <bytes> (<pct>%) <dur> left`; an error tail may follow.
Bytes are base-10 human units (`140 MB` = 140_000_000), so the byte figures are rounded (~3 sig figs) — accepted, per the spec's "coarser resolution mid-run".
Struct `KopiaProgress { hashing, files_hashed, bytes_hashed, files_cached, bytes_cached, bytes_uploaded, bytes_estimated: Option, errors: Option, ignored_errors: Option }`.
Parser returns `None` for non-progress stderr (maintenance lines etc.).
Fixture tests against the real formats above, including the estimated-percent form and an errors form.

kopia's summary line carries no current-path, so `current_path` is not populated from it (omitted).

### 3. Proxy traffic handle — `crates/kopia/src/proxy.rs`

Add a cheaply-cloneable `TrafficHandle` (newtype over the existing `Arc<Traffic>`) with `.snapshot() -> TrafficStats`, and `RunningProxy::traffic_handle()`.
The atomics are already lock-free and monotonic, so a concurrent sampler reading them mid-run is safe.

### 4. Freeze timestamp — `crates/bestool/src/actions/canopy/backup/`

Add `taken_at: Option<Timestamp>` to `Prepared` (`method.rs`).
Capture `Timestamp::now()` at each atomic-snapshot instant and carry it out:
- btrfs — after `btrfs subvolume snapshot -r` succeeds (`postgresql/btrfs.rs`)
- thin-LVM — after `lvcreate --snapshot` succeeds (`postgresql/lvm.rs`)
- VSS — after `Win32_ShadowCopy.Create` succeeds (`postgresql/vss.rs`)
Leave it `None` for the base-backup fallback (streamed interval) and the simple method (live view).
The fallback path already retags as basebackup, so a snapshot backend that degrades to basebackup correctly reports no freeze moment.

### 5. Always stream `--progress` + shared progress cell

kopia is currently run with `--progress` and streamed only when a local CLI is attached (the mpsc sink).
Always stream and parse now, so Canopy gets engine counters on every run (daemon or `--no-daemon`); the local `BackupEvent::Progress` emit stays as-is for CLI display.
Introduce `Arc<ProgressCell>` holding the latest parsed `KopiaProgress` and the freeze instant; the stderr parse loop updates the kopia counters, and `run_kopia_backup` writes the freeze instant into it after `prepare`.

### 6. Progress sampler — `crates/bestool/src/actions/canopy/backup/progress.rs`

A shared helper spawned around the long phase (the snapshot upload for backup; the restore download for restore).
Inputs: canopy client, run uuid, type, purpose, proxy `TrafficHandle`, and an optional `Arc<ProgressCell>` (present for backup, absent for restore).
Fires an immediate first sample (carrying `snapshot_taken_at` as soon as `prepare` has set it), then every ~30s reads the cell + traffic handle, builds cumulative `ProgressArgs`, and POSTs.
The verbatim kopia status line goes in `extra`.
Every POST error is logged at debug and swallowed.
Stopped when the phase ends (a stop signal; the task is awaited so it can send nothing after the run is torn down).

Backup: mapping kopia → Canopy is `bytes_hashed`, `bytes_uploaded`, `bytes_cached`, `bytes_estimated`; `files_done = files_hashed + files_cached`; `errors`, `ignored_errors`; `bytes_read` and `files_estimated` and `current_path` omitted (not exposed by the line).
Restore: proxy tallies only, `purpose = restore`, no cell, no freeze moment. Restore keeps its inherited-terminal kopia display (its output isn't captured), so restore engine counters are out — the S3 received-bytes tally is the "is it moving" signal.

### 7. Report the freeze moment

Attach `snapshot_taken_at` to the backup `ReportArgs` (read from the progress cell, so it's present on both success and failure once `prepare` ran).
Restore reports no freeze moment.

### 8. Contract tests — `crates/bestool/src/canopy_contract.rs`

Add `backup_progress_request_matches_spec` (a populated `ProgressArgs` validates against the live spec; a negative case for an invalid purpose).
Add `snapshot_taken_at` to the report test's `ReportArgs`.

## Testing

- Parser: unit tests over the real kopia line formats (estimating, estimated-percent, errors, cached-only, non-progress lines → None).
- Freeze timestamp: a snapshot strategy sets `taken_at`; basebackup and simple leave it `None`.
- Sampler mapping: a `KopiaProgress` + `TrafficStats` produce the expected cumulative `ProgressArgs` with the right omissions.
- Sampler resilience: a failing POST does not propagate (the run's own result is unaffected).
- Contract tests as above (the dedicated CI job).

## Deploy note

Once bestool starts sending `snapshot_taken_at`, Canopy dates a run's data from when it was frozen rather than from its report time.
A server whose backup takes many hours will therefore read as measurably staler than before — more truthful, but a state change for servers sitting near their staleness threshold.
The shift is per-server as each host upgrades bestool; nothing moves for a run that reports no freeze moment.

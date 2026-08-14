# Held captures

Implementation plan for [HOLD](../specs/canopy/held-captures.md).

## Design notes

### Holding is a promotion, not a skipped teardown

Every backend names and places its capture for the *run*: a stable per-backup-type mount path, and (btrfs/LVM) a `bestool-kopia-` infix that exists so `reap_stale` can glob orphans at the start of the next run.
Leaving a capture under those names and paths means the next run of the same type unmounts it (VSS) or destroys it outright (btrfs, LVM).

So each backend gets a `hold` operation that promotes its capture out of run-owned namespace:

| backend | promotion |
| --- | --- |
| btrfs | rename the snapshot subvolume to `bestool-held-<id>`, remount read-only at the hold path, keep the top-level mount |
| thin-LVM | `lvrename` the snapshot LV to `bestool-held-<id>`, remount at the hold path |
| VSS | junction the shadow at the hold path, drop the run's junction, keep the shadow |
| base backup | rename the staging tree to the hold path |
| simple | nothing to promote — the capture is a live view, not a point-in-time copy |

`simple` holds are refused with that reason: a bindfs view or live path has no freeze instant (it already reports `taken_at: None`), so retaining it would offer a rollback point that isn't one.

### Release is the promotion's inverse

A hold record carries enough to release the capture without the run that made it.
Release reconstructs the backend's teardown state from the record and runs the same teardown, so there is one release path per backend rather than two.

### Layout

- record: `/var/lib/bestool/held-snapshots/<id>.json` (Unix), `%ProgramData%\bestool\held-snapshots\<id>.json` (Windows)
- mount: `/var/lib/bestool/held-source/<id>` (Unix), `<vol>\bestool-backup-shadow\held\<id>` (Windows — must sit on the shadow's own volume for the junction)
- id: `<type>-<YYYYmmddTHHMMSSZ>`, from the freeze instant — sortable, typeable, and meaningful in `hold list` output

## Checklist

### 1. Hold records and storage
- [ ] Make the teardown payloads (`simple::Cleanup`, `btrfs::Mounts`, `lvm::Snapshot`, `vss::Shadow`, base-backup root) serialisable, with accessors to rehydrate them
- [ ] `backup/hold.rs`: `HoldRecord` (id, type, `taken_at`, source path, uploaded, held-at, backend descriptor), the records directory, save/load/list/remove
- [ ] `release(record)` — rehydrate and run the backend's teardown; treat an already-absent capture as success
- [ ] Tests: round-trip a record per backend; release of a missing capture succeeds

### 2. Promotion per backend
- [ ] `hold` on each of btrfs / lvm / vss / basebackup, per the table above
- [ ] Refuse to hold a `simple` capture, naming the reason
- [ ] Confirm the held names fall outside `reap_stale`'s globs in btrfs and lvm
- [ ] Tests: held names don't match the reaper globs; path helpers

### 3. Run plumbing
- [ ] `RunControl` (hold flag) shared into the run alongside `ProgressCell`
- [ ] Cleanup site promotes instead of tearing down when held, and writes the record
- [ ] `--hold` and `--no-upload` on `bestool canopy backup`; `--no-upload` skips target, credentials, proxy, kopia, and reporting, but keeps `pre`/`post` hooks and preparation
- [ ] Tests: `--no-upload` reaches neither Canopy nor kopia; a held run records what it held

### 4. Mid-run keep
- [ ] Registry holds each run's `RunControl`; endpoint to set the hold flag on the in-flight run for a type
- [ ] `bestool canopy hold keep --type X`, reporting plainly when no daemon-hosted run is in flight
- [ ] Tests: setting the flag mid-run is observed at cleanup; unknown type is a clean report

### 5. Hold commands
- [ ] `bestool canopy hold list` — id, type, freeze instant, age, uploaded, capture present
- [ ] `bestool canopy hold drop <id>` — release and remove; a vanished capture removes the record and says so
- [ ] Tests: list rendering; drop of a vanished capture

### 6. Restore from a hold
- [ ] `--from-hold <id>` on `bestool canopy restore`: copy the capture into the restore staging area, then the existing method restore unchanged
- [ ] Space precheck sized for the staged copy plus the retained previous tree; refuse up front naming needed and free
- [ ] Tests: precheck refuses when short and names the figures; the hold survives a restore

### 7. Reporting check
- [ ] alertd check over the hold records: age, and whether each capture is still present
- [ ] Report the platform snapshot store's headroom where it is bounded and shared (Windows); never change it
- [ ] Tests: grading for old, for vanished, and for a clean device

### 8. Finish
- [ ] `cargo clippy`, `cargo fmt`, `cargo check` for a Windows GNU target
- [ ] `./update-usage.sh` squashed into the help-text commit
- [ ] Fold the plan into the spec and delete it (`unplan:`)

## Open questions

- The spec has `hold keep` work only for daemon-hosted runs. A `--no-daemon` run started before an upgrade has no escape hatch. Left as specified.
- Restore-from-hold peaks at roughly twice the cluster size (staged copy + retained previous tree). On a host at the low end of typical free space the precheck will always refuse, leaving the operator to clear space by hand. Whether bestool should offer a no-retain restore is undecided and out of scope here.

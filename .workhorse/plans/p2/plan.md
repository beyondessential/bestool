# Exercise the hold lifecycle end to end on every backend

The `vss / wmi e2e` job exercises the pieces of a VSS capture but never the
operator workflow that strings them together, which is why a hold that reported
DETACHED from the moment it was taken shipped and stayed shipped (N2). `hold
list` and `restore --from-hold` have no coverage on any backend.

The work is one shared lifecycle driver, run against real storage and a real
postgres cluster on each of the four backends, plus the CI jobs that stand each
backend up.

## Shape

One `#[ignore]`d test per backend in `actions/canopy/hold/e2e.rs`, each building
its own storage and cluster in Rust (as the existing `btrfs::e2e` tests do) and
then calling a single shared `lifecycle` driver. CI installs the packages each
backend needs and runs one test-binary filter under sudo, so the YAML stays short
and the fixtures stay beside the assertions they set up.

### Layer

Settled with the user: drive the command entry points (`backup::run`,
`hold::run`, `restore::run`) with the argument structs clap would have built,
rather than spawning the `bestool` binary. This is the operator's sequence and
the real dispatch, including the listing and the restore's capture-state gate;
what it leaves uncovered is clap wiring itself.

### Restore depth

Settled with the user: the full restore, including the method's lay-down — the
service stop, the directory swap, and the start. Each backend's fixture
therefore builds a genuine cluster that the restore can stop and start:

- Linux: `pg_createcluster <ver> <name> -d <datadir>`, which yields the
  `postgresql@<ver>-<name>` systemd unit `service::stop`/`start` drive.
- Windows: the runner's preinstalled server. Its data sits at `C:\PostgreSQL\<ver>\data`,
  which has no sibling `bin`, so `plan_restore` classifies the capture
  data-only and the restore replaces the data directory rather than the whole
  install.

## The lifecycle

Driven once per backend, in the operator's order:

1. Marker written into the live cluster carrying the *frozen* value.
2. `backup --type X --hold --no-upload` — capture-only hold.
3. Marker overwritten with a *live* value, so the freeze-vs-live assertion
   afterwards can distinguish them.
4. `hold list` succeeds, and the record for our type reports `Present` — not
   `Detached`, not `Gone`. This is the step that would have caught N2.
5. The capture reads at the path the record names, and carries the frozen value.
6. `restore --type X --from-hold <id>` completes.
7. The cluster is up again and its data dir carries the frozen value, not the
   live one.
8. The hold survives the restore: still listed, still `Present`.
9. `hold reattach` on a healthy hold is a no-op that still reports readable —
   except on base backup, which exposes nothing separately and must refuse.
10. `hold drop` releases the underlying capture — subvolume / LV / shadow copy
    actually gone, not just the record — and frees its space.

### The VSS stale-junction case

A hold taken before a reboot names a `HarddiskVolumeShadowCopyN` device that VSS
may renumber, which is the case `reattach` exists for and which nothing
exercises. A runner cannot reboot mid-job, so the junction is repointed at a
bogus device number to stand in for it. Then: the hold reports `Detached`,
`restore --from-hold` refuses naming that state, `hold reattach` puts it back,
and the capture reads again.

## Detecting rather than forcing the backend

No fixture sets a `strategy` override. Each one builds storage the detection
already reads the way a real host's does, and the lifecycle asserts which backend
the hold ended up on. That way a snapshot backend that fails and falls back to a
base backup shows up as a failed assertion naming both, instead of a job quietly
testing a backend it does not mean to.

## Asserting the space is freed

"Gone, not just the record" is asserted per backend against the storage itself —
`btrfs subvolume list`, `lvs`, WMI `Win32_ShadowCopy`, directory absence.

For the free-space delta, a snapshot of a freshly-created cluster shares all its
extents with the live data, so dropping it frees nothing measurable. The
fixtures therefore write a ballast file before the capture and overwrite it
after, so the capture pins the old extents and dropping it returns them. On
btrfs and thin-LVM the filesystem is ours alone and the delta is assertable.

The ballast has to be rewritten a third time, after the restore, and this is the
part that is easy to get wrong — the first CI run failed on exactly it. The
restore copies the capture back into the live tree with `cp`, and on btrfs that
reflinks: the copy shares the capture's extents rather than allocating its own.
The capture then pins nothing of its own, and dropping it frees nothing. That is
the filesystem behaving correctly, not a hold failing to release, so measuring
across it asserts the opposite of what it appears to. Rewriting the live copy
breaks the sharing and leaves the capture the only claim on what it froze.
Measured on a loopback filesystem: 192 MiB in use before the drop, 128 MiB after.

thin-LVM never had the problem — its snapshot is block-level, so the restore's
copy allocates fresh pool blocks and the snapshot's stay unique. That is why the
thin-LVM job passed on the run where btrfs failed.

Reclaiming is not synchronous either. btrfs unlinks a deleted subvolume at once
but frees its extents on the cleaner thread, which took around 30 seconds for a
capture this size, so the assertion polls rather than reading once.

On VSS the store is the runner's system volume, which other processes are
writing to throughout, so a byte delta there would be flaky; the shadow's
absence from WMI is the assertion instead, since that is what returns its store.
Recorded in the test cases as covered-by-absence rather than left unticked.

The ballast is written only where it is read. On base backup it would be streamed
through `pg_basebackup`, walked to size the restore, and copied again into
staging; on VSS it would grow the shadow's store on the runner's system volume.
Neither measures a delta, so neither pays for it.

The thin pool's size is derived from the ballast rather than fixed, and so are
both margins. The pool has to hold several ballast-sized allocations at once —
what the capture pins, the live copy, the restore's staged copy, the tree it
displaces — and an ext4 volume mounted without `discard` never hands blocks back.
Left decoupled, raising the ballast or adding a step to the shared driver would
fill the pool and flip the filesystem read-only instead of failing an assertion.

## Build steps

- [x] `hold/e2e.rs`: the `Backend` trait and the shared `lifecycle` driver
- [x] btrfs fixture: loopback image, subvolume, cluster, `lifecycle`
- [x] thin-LVM fixture: loopback PV, thin pool, thin LV, cluster, `lifecycle`
- [x] base backup fixture: plain cluster on the machine's own disk
- [x] VSS fixture: the runner's cluster, `lifecycle`
- [x] VSS stale-junction case
- [x] CI: btrfs lifecycle step on the existing `btrfs hold / e2e` job
- [x] CI: `thin-lvm hold / e2e` job
- [x] CI: `base backup hold / e2e` job
- [x] CI: VSS lifecycle steps on the existing `vss / wmi e2e` job
- [x] CI: add the new jobs to `tests-pass`
- [x] `cargo clippy --all-targets --all-features` and `cargo fmt`
- [x] Windows GNU target `cargo check` for the VSS-side changes. The default
      feature set does not build for that target on a Linux host — a transitive
      dependency's build script needs a Windows resource compiler — so the check
      runs against `--no-default-features --features canopy,self-update`, which
      covers all of this work

## Notes

- Hold records and held-source paths are fixed under `/var/lib/bestool` (and
  `%ProgramData%` on Windows), so every step needs root/admin and the backends
  share those directories. The tests run `--test-threads=1`, and each asserts on
  its own hold id rather than on the whole listing.
- `capture_only` takes the same per-type lock an uploading run does, so each
  backend uses a backup type name of its own.

## What is not verified locally

The btrfs fixture's storage half was run by hand on a loopback filesystem: the
`by-uuid` link resolves for a loop device, the data directory's mountpoint is the
subvolume the capture snapshots, the snapshot mounts by name and reads, and
`btrfs subvolume list` reflects a delete in the shape `capture_present` parses.

The rest is first exercised in CI. The cluster fixtures are Debian-shaped —
`pg_createcluster` and the `postgresql@<version>-<cluster>` unit — as the
production restore path itself is, so they cannot run on a non-Debian
workstation. The thin-LVM and VSS storage assumptions have no local equivalent
either.

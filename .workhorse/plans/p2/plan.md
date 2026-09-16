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
`lvs`, WMI `Win32_ShadowCopy`, directory absence, and on btrfs a mount of the
filesystem's own top level.

btrfs is asked by mounting subvolid 5 and looking for the subvolume directly,
rather than by reading `btrfs subvolume list`. A held snapshot sits beside the
cluster's subvolume rather than under it, and the listing scopes and formats its
output according to where it is run from and which version is installed — which
made it the wrong instrument for a question that has to have the same answer on
every host. The probe also answers while the capture is still held, so a probe
looking in the wrong place fails loudly instead of letting the check after the
drop pass without reading the storage.

The claim is made on the capture, not on the filesystem around it. A btrfs
fixture enables quota groups before anything is written, and the assertion reads
the held subvolume's *exclusive* bytes while the hold is still in place: storage
nothing else references is storage that deleting the capture necessarily
returns, so that reading together with the capture being gone afterwards says
what the drop returned. Nothing about the filesystem's free space enters it.

A snapshot of a freshly-created cluster shares everything with the live data and
so holds nothing exclusively. The fixtures therefore write a ballast file before
the capture and rewrite it whenever the two need to stop sharing storage: once
after the capture, and again after the restore, which copies the capture back
into the live tree with `cp` and on btrfs reflinks it — the copy shares the
capture's extents rather than allocating its own, and the capture is left
holding nothing of its own. Measured on a loopback filesystem carrying a
cluster-shaped tree of 3000 small files: 64.1 MiB exclusive, against a margin of
half the ballast.

thin-LVM cannot account per capture — a thin pool reports what it has mapped,
not what one snapshot holds alone — so it reads the pool either side of the
release instead. The pool is the fixture's alone, which is what makes that
readable at all, and the job has passed on it throughout.

### Why the filesystem's free space was the wrong instrument

The btrfs assertion went through three CI failures reading free space before it
was rebased on exclusive accounting, and the wrong readings are worth recording
because they look right.

`statvfs` — free space through the ordinary interfaces — is not it. btrfs reports
free space net of the chunks it has allocated, and a metadata chunk on a single
device is duplicated, so allocating one moves the number by hundreds of megabytes
that freeing data never brings back. A cluster sitting beside its own restored
copy makes enough metadata to allocate one, and a 64 MiB signal does not survive
that. `btrfs filesystem df --raw` counts the data extents themselves and is
untouched by it, but it is still a filesystem-wide number, still moved by
whatever else writes, and still waiting on the cleaner thread to return a deleted
subvolume's extents.

Ruled out along the way, each reproduced on a real loopback filesystem and each
behaving correctly: the held-source mount pinning the subvolume past its
deletion, a live cluster's copy-on-write churn masking the delta, and the cleaner
being slower on a real disk than on the tmpfs the first reproductions
accidentally ran on. The reproductions were themselves misleading, because a
single large ballast file makes almost no metadata — which is precisely the
variable that mattered.

The cluster is still stopped before the reading, and a btrfs release still waits
on `btrfs subvolume sync`, so the capture's absence is asserted against a settled
filesystem rather than one mid-cleanup.

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

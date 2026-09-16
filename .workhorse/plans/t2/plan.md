# Restore in place, from a hold, without room for a second copy

`restore --from-hold` stages a whole second copy of the capture before laying it down, so it needs free space for another cluster and, once `replace_dir` has left `<dest>.old` behind, about 1.5× the cluster to finish.
On a single-volume host with a 732 GiB capture and 82 GiB free, the rollback point exists, is readable, and cannot be used.

This adds `--in-place`: sync the capture onto the live tree, copying only what diverged.

## The shape of the solution

The divergence between a hold and the cluster it rolls back onto is hours of writes, not the size of the database, so the cost is proportional to the delta rather than to the capture.

### A metadata walk decides structure; a basis decides content

The expensive thing is reading file *contents*: 732 GiB on each side.
Reading *metadata* — `readdir` plus `stat` — is cheap even on a large tree, and it already answers every structural question:

- present in the capture, absent from the live tree → copy
- absent from the capture, present in the live tree → delete
- present in both, different type → delete and copy
- present in both, different size → copy

That leaves exactly one undecided case: **present in both, same size**.
A diff basis decides that case, and nothing else.
This is why the basis is never load-bearing for correctness — a missing, stale, or wrapped basis degrades to the fallback rule and the restore is still complete.

Deletions come from the walk, not from the basis.
That matters because `btrfs subvolume find-new` cannot report deletions (a deleted file leaves no inode with a newer generation), and reaching for `btrfs send` to get them would mean parsing a binary stream for information the walk already has.

### The decision table for a same-size file

| | mtime identical | mtime differs |
|---|---|---|
| no basis (fallback) | skip | hash both sides, copy if they differ |
| basis available | copy iff the basis names it | copy iff the basis names it |

The fallback's skip-on-identical-size-and-mtime is a **degraded mode**: a file that diverges without changing either is silently kept, which on a data directory is corruption that surfaces later as a bad page.
It is acceptable only because it is the fallback — where a basis is available it is authoritative and the fallback rule never runs.
The mode says which of the two it used, so an operator reading the log knows which guarantee they got.

Hashing every same-size file regardless of mtime would close that gap, at the cost of a full read of both sides — the 732 GiB the whole design exists to avoid.

## Per-backend diff bases

Each backend can name the diverged set from metadata, without reading contents.

- **btrfs** — `btrfs subvolume find-new <live-subvol> <generation>` lists files written since a transaction generation. File-level, so it drops straight into the table above. The hold record has to carry the generation at capture, which it does not today.
- **VSS** — the NTFS USN change journal enumerates what changed since a recorded USN. File-level. The journal is a fixed-size **ring**, so a wrap since the capture means the answer is partial; a partial answer is not a basis, so a detected wrap yields no basis and the fallback runs. So does a journal that has been deleted and recreated, which restarts its numbering. The record has to carry the journal id and USN at capture. Verify on a real host before relying on it.
- **thin LVM** — `thin_delta` diffs two thin devices' block mappings. Block-level, and there is no reverse map from a block to the file that owns it, so it **cannot** decide the same-size case. What it can do soundly and cheaply is *size* the divergence for the space gate, and answer "nothing diverged at all" outright. It contributes that and not a path set.

## Space: two different resources

`ensure_free_space` gates on the size of the capture, which is the wrong question for this path — running it here refuses a restore that fits.
In place, two resources are consumed and they are not the same number:

- **filesystem free space** — the *net* growth: bytes added by new and grown files, less bytes freed by deleted and shrunk ones. Usually small, often negative.
- **copy-on-write store** — the *total* bytes written, because every block written over is a block the snapshot still references and so is copied aside first. On VSS this is the shadow storage diff area; when it hits its `vssadmin` cap, VSS deletes shadows to make room, and there is only one. That failure mode — the source vanishing partway through a copy that has already overwritten the destination — is why the naive whole-tree in-place copy is unsafe and why this mode copies only the delta.

## What this gives up, and how that is made safe

There is no `.old` to fall back to, and mid-copy the tree is neither the old state nor the captured one.
The hold survives (reads do not consume COW), so a retry from the same rollback point is the documented recovery — and because the sync is idempotent, a retry resumes rather than restarts.

A half-restored data directory that someone starts is corrupt, so the mode makes the cluster physically unstartable for the duration: `PG_VERSION` is renamed aside before the first write and written back from the capture last, and postgres refuses to start without it.
A marker file alongside it records what is in flight, so the state is legible rather than merely broken, and so a resumed run knows it is resuming.

## Build steps

### The sync engine

- [x] Metadata walk of both trees producing a structural delta: copy / delete / recurse / undecided-same-size
- [x] Fallback rule for the undecided set: skip on identical mtime, else hash both sides
- [x] Apply the delta: copy files preserving mode, ownership and mtime; delete extras; create and remove directories
- [x] Report progress — a long sync is otherwise a silent wait, as the repository restore path already learned
- [x] Idempotent: re-running over a partially synced tree converges

### The basis seam

- [x] A basis type that is either a path set or absent, resolved per backend from the hold record
- [x] A divergence estimate, separately sourced, for the space gate
- [x] Log which basis was used, and say plainly when the fallback's degraded rule is in play

### Backends

- [x] btrfs: carry the generation on the hold record at capture; `find-new` against the live subvolume
- [x] VSS: carry the journal id and USN on the hold record at capture; read the journal, detect a wrap, yield no basis on wrap
- [x] thin LVM: `thin_delta` for the divergence estimate and the nothing-changed fast path

### Space gate

- [x] Net-growth check against filesystem free space, replacing the capture-sized check on this path
- [x] COW-store check: shadow storage headroom on VSS, pool free space on thin LVM
- [x] Name `--in-place` in the ordinary path's shortfall message, so an operator who hits the wall is told the way through it

### Safety

- [x] Rename `PG_VERSION` aside before the first write; write it back from the capture last
- [x] Marker file recording hold id, capture path and start time
- [x] Refuse to start the cluster while the marker is present
- [x] On failure, bail naming the state and the resume command rather than leaving it silent

### Surface

- [x] `--in-place` on `restore`, requiring `--from-hold`
- [x] Accepted by every method (the secret key's single file makes it a no-op in practice, but it is not an error)
- [x] Spec: fold into [HOLD](../../specs/canopy/held-captures.md)
- [x] Test cases for the card

## What is verified, and what is not

Verified end to end on this machine, against the `simple` method with a staged capture: a diverged file rolled back, a post-freeze file removed, an unchanged file not rewritten, the hold untouched, a second run converging to no work, and the refusals (empty capture, missing confirmation, `--in-place` without a hold, `--in-place` with a snapshot id).

Not verified, and needing real storage:

- **btrfs** — that the generation recorded at capture makes `find-new` name exactly the files written since. The parser is tested against representative output; the generation capture and the `find-new` invocation are not.
- **thin LVM** — `thin_delta` against a real pool. `reserve_metadata_snap` needs privileges and tooling that may be absent, which is why every failure here degrades to the walk's estimate rather than refusing.
- **VSS** — the whole change-journal path. It is read through `usn-journal-rs`, which wraps the `DeviceIoControl` calls and reconstructs each record's path from its parent's file id; this workspace forbids unsafe code, so a crate that encapsulates it is the way in. Whether the journal is active, whether the recorded position survives the ring, and whether the reconstructed paths land where the restore expects them all need a real host. Treat as an optimisation behind the fallback until one confirms it.
- **postgres** — that a restored cluster starts and verifies, on each backend. The in-place path reuses the staged path's stop/ownership/start/verify sequence, but the sequence around the interlock is new.

These belong with [P2](https://github.com/beyondessential/bestool/pull/898)'s per-backend hold lifecycle jobs, which already have the hosts.

## Still outstanding

Raised in review, real, and not done here — each is a contained follow-up rather than a correctness gap:

- **Batching the copy loop.** The removal, directory and metadata passes each run in one blocking task; the copy loop still dispatches per entry. Draining it in chunks would amortise the scheduling without changing ordering or copy-on-write behaviour. Distinct from the concurrency question below.
- **Resolving the thin pool once.** `basis::lvm::diverged_bytes` and `room::thin_pool_store` each resolve `pool_lv` for the same LV, so a thin-LVM plan shells out to `lvs` about twice as often as it needs to. `blockdev`'s module doc already promises this and does not deliver it.
- **One shell-out layer.** `blockdev` is the third wrapper around `findmnt`/`lvs` in this crate, alongside `postgresql::sys` and `strategy.rs`. The `--` and device-mapper-mangling correctness it centralises is still missing from the two older copies; promoting `sys` to a neutral module both sides use is the fix.
- **A neutral home for free-space probing and byte formatting.** `room` reaches into `backup::postgresql::space`, which is why `fmt_bytes` had to widen to `pub(crate)`. Neither is a postgres concept.
- **The whole-tree delta.** When the destination does not exist, the walk puts the capture's entire file list in the delta before anything is written. That is the documented "laid down whole" case and is not what this mode exists for, but it is unbounded.

## Deliberately not done

- **Moving the change-journal reader out from under `backup/postgresql/`.** Review is right that it is NTFS-generic rather than postgres-specific, and that the generic restore engine reaching across for it points the dependency the wrong way. But its other caller is the VSS backend at capture time, which does live there, so moving it under `restore/` inverts the same arrow rather than removing it. A neutral home is a wider reorganisation than this card, and worth doing when a second platform needs one.
- **A `LayDown` enum unifying `Method::restore` and `Method::restore_in_place`.** Review's stated risk — a new method added to one match and forgotten in the other — does not hold: a new `Method` variant fails to compile in both, because both matches are exhaustive. The genuinely shared part, resolving where the secret key is laid down, is now one function; the rest of each arm differs in ways an enum would only re-encode.

- **Concurrent copies and removals in `apply`.** Review suggested driving both loops through a bounded `buffer_unordered`. Removals cannot: they are ordered children-before-parents, and an unordered pool would try to `rmdir` a directory before its contents. Copies could, but the win is speculative on the single volume this mode exists for, where the writes are already sequential and the copy-on-write store is the bottleneck rather than the scheduler. Folding the whole per-entry operation into one `spawn_blocking` (done) removes the overhead that was actually measurable in the structure. Worth revisiting with a real host and a real delta, not before.

## Open questions

- Does `thin_delta` reach the pool metadata without `reserve_metadata_snap` privileges the daemon may not hold? If not, the estimate degrades to the walk's, which is not a failure.
- The card weighed the fallback walk against "trusting size+mtime alone", but the decided rule *is* that for the skip decision. Settled as: the skip is acceptable **because** it is the fallback, and the mode says when it is in play. If that turns out not to be good enough on a data directory, the change is to hash every same-size entry when no basis is available.

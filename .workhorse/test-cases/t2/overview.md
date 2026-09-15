# Restoring from a hold in place

Scenarios that verify `restore --from-hold --in-place`.

The unticked ones are coverage this card owes and does not yet have: they need real storage — a btrfs subvolume, a thin pool, a Windows volume with an active change journal, a running cluster — and belong with the per-backend hold lifecycle jobs P2 is building.

## Rolling back

- [x] A file that diverged since the freeze is rolled back to its captured contents
- [x] A file created since the freeze is removed
- [x] A file deleted since the freeze comes back
- [x] A file unchanged since the freeze is not rewritten
- [x] A directory created since the freeze is removed, contents before the directory
- [x] An empty directory in the capture comes back
- [x] A symlink is restored as a link, not as a copy of what it points at (verifies spec: HOLD)
- [x] An entry that changed kind between the two sides is replaced rather than written through
- [x] The hold's capture is unchanged by the restore (verifies spec: HOLD)
- [ ] The restored cluster starts and passes its verification, on each backend

## Deciding what diverged

- [x] Differing sizes settle a file without reading either side
- [x] Same size and same modification time is skipped without reading — the fallback's degraded rule (verifies spec: HOLD)
- [x] Same size, differing modification time, same contents is not copied
- [x] Same size, differing modification time, differing contents is copied
- [x] btrfs `find-new` output is read as the set of changed paths, re-based onto the restored tree
- [x] btrfs `find-new` reporting nothing changed is an empty set, not an absent basis
- [x] Paths on the subvolume but outside the restored tree are left out
- [x] The NTFS change journal's records are read as names under parent directories
- [x] A recreated change journal (different id) yields no basis
- [x] A change journal wrapped past the capture yields no basis (verifies spec: HOLD)
- [x] `thin_delta` output is summed over every kind of difference and no sameness
- [ ] On a real btrfs host, the generation recorded at capture names exactly the files written since
- [ ] On a real Windows host, the journal names exactly the files written since the shadow
- [ ] On a real thin pool, `thin_delta` sizes the divergence against a known write volume

## Space

- [x] Net growth nets removals and displacement off against what is copied
- [x] A delta that frees more than it adds needs no room
- [x] Where the capture shares the live filesystem, the whole written volume is charged to it, not the net
- [x] A separate copy-on-write store without room refuses and names how to raise it
- [x] A separate store whose headroom cannot be read warns rather than refusing
- [x] `vssadmin` shadow storage headroom is read as the cap less what is used
- [x] An unbounded shadow storage cap is not gated on
- [x] Unrecognised `vssadmin` output yields no number rather than a wrong one
- [ ] On the motivating host shape — capture larger than free space — the staged path refuses and the in-place path succeeds

## Safety

- [x] An empty capture is refused before anything is written, leaving the live tree intact
- [x] An absent capture is refused
- [x] The interlock refuses a start while it is engaged, and permits one after release
- [x] The marker names the hold to resume from
- [x] An interrupted restore is finished by running the same command again
- [x] Applying the delta twice converges
- [x] A tree that already matches the capture is reported as no work
- [x] Restoring in place without confirmation is refused, even onto an empty destination
- [x] `PG_VERSION` is parked out of the way before the first write and written back from the capture last
- [x] The parked `PG_VERSION` and the marker are kept out of the sync, which would otherwise remove them as post-freeze files
- [x] A resumed restore keeps the pre-restore `PG_VERSION` parked rather than adopting a partial one
- [x] Parking an absent `PG_VERSION` is not an error
- [ ] Postgres refuses to start against a part-restored cluster on a real host
- [ ] Killing a restore mid-sync leaves the marker, and the service does not come up on reboot

## Surface

- [x] `--in-place` without `--from-hold` is refused
- [x] `--in-place` with a snapshot id is refused
- [x] The staged path's shortfall message names `--in-place` as the way through
- [x] A hold record written before the divergence mark existed still parses
- [x] Every backend's hold record round-trips with its mark

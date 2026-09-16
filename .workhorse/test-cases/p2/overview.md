# Hold lifecycle coverage

The operator's sequence, run against real storage and a real postgres cluster on
each backend. One shared driver holds every backend to the same assertions, so a
case ticked below is ticked on all four unless it says otherwise.

Backends: btrfs and thin-LVM (`btrfs hold / e2e`, `thin-lvm hold / e2e`), base
backup (`base backup hold / e2e`), VSS (`vss / wmi e2e`).

## Taking the hold

- [x] `backup --type X --hold --no-upload` takes a capture and reports the run
      as not uploaded (verifies spec: HOLD)
- [x] The hold record names the backend the storage called for, rather than the
      base-backup fallback a failed snapshot would have left
- [x] The definition's method is resolved from the def's own backups directory,
      so the fixture's cluster is the one captured

## The listing

- [x] `hold list` succeeds with a hold on the device (verifies spec: HOLD)
- [x] The hold reports its capture present, not detached and not missing, from
      the moment it is taken — the case that shipped broken on VSS
      (verifies spec: HOLD)
- [ ] The rendered listing carries the id, backend, freeze instant, age, upload
      flag and state for each hold. The lifecycle runs the command and asserts it
      succeeds, but reads the state through the record rather than off stdout, so
      the table's columns themselves are unasserted

## Reading the capture

- [x] The capture reads at the path its record names, and carries the value as
      it stood at the freeze (verifies spec: HOLD)

## Restoring from the hold

- [x] `restore --type X --from-hold <id>` completes, including the method
      stopping the cluster, swapping the tree into place and starting it again
      (verifies spec: HOLD)
- [x] The restored cluster carries the value from the freeze, not the value
      written to the live cluster afterwards (verifies spec: HOLD)
- [x] The cluster accepts connections after the restore
- [x] The hold outlives the restore that read it, and is still present
      (verifies spec: HOLD)
- [ ] A restore refuses up front when there is not room to stage a second copy
      of the capture, naming what is needed and what is free
      (verifies spec: HOLD)

## Reattaching

- [x] Reattaching a healthy hold changes nothing and still reports the capture
      readable — btrfs, thin-LVM, VSS (verifies spec: HOLD)
- [x] Base backup refuses a reattach, since nothing exposes its capture
      separately (verifies spec: HOLD)
- [x] VSS: a hold whose junction points at a device number VSS has not handed
      out — the state a reboot leaves one in — reports detached, not gone
      (verifies spec: HOLD)
- [x] VSS: a restore from that hold refuses, naming the state and the reattach
      that recovers it, rather than laying an empty tree over the cluster
      (verifies spec: HOLD)
- [x] VSS: reattaching rebuilds the junction from the shadow id, and the capture
      reads again (verifies spec: HOLD)
- [ ] The same detached → refuse → reattach sequence on btrfs and thin-LVM,
      whose captures detach when the mount that exposes them goes with the
      process that made it. The pieces have unit and e2e coverage at the method
      level; the operator sequence over them does not

## Dropping

- [x] `hold drop` removes the record (verifies spec: HOLD)
- [x] The capture behind it is gone from the storage, not just the record: the
      subvolume off the filesystem, the logical volume off the pool, the shadow
      copy out of VSS, the staged tree off the disk (verifies spec: HOLD)
- [x] The probe that judges the above answers "present" while the capture is
      still held, so a probe that looked in the wrong place could not let the
      post-drop assertion pass without reading the storage
- [x] btrfs: the capture holds storage of its own for the drop to return. Quota
      groups are enabled before anything is written, and the held subvolume's
      exclusive bytes are read while the hold is still in place — storage nothing
      else references, which deleting the capture necessarily returns. Read
      together with the capture being gone afterwards (verifies spec: HOLD)
- [x] thin-LVM: the pool's usage falls across the release. A thin pool cannot say
      what one snapshot holds alone, so the pool — which is the fixture's own —
      is read either side instead (verifies spec: HOLD)
- [x] VSS and base backup: the space comes back by the capture's absence — a
      deleted shadow copy returns its store, and a removed tree returns its bytes
      — rather than by a byte delta. The VSS store is the machine's system
      volume, which the rest of the machine writes to throughout, so a delta
      there would be noise. Neither writes the ballast, since nothing reads it
      there (verifies spec: HOLD)
- [ ] Dropping a hold whose capture has already gone removes the record and
      reports the capture absent, rather than failing (verifies spec: HOLD)

## Not covered by this card

- [ ] `hold keep` telling a daemon-hosted run in flight to retain its capture
      (verifies spec: HOLD)
- [ ] An uploading run asked to hold (`backup --hold` without `--no-upload`)
      leaving both a snapshot in the repository and a hold on the device
      (verifies spec: HOLD)
- [ ] The doctor check reporting long-held and vanished holds, and the snapshot
      store's headroom (verifies spec: HOLD)

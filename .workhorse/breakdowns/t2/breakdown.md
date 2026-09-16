# Follow-ups from restoring in place

One piece of work that reviews of T2 raised, that is real, and that does not belong in a restore card.

Everything else the reviews surfaced was either fixed in T2 or is a reasoned decision recorded in its plan. The verification the mode still owes — btrfs, thin LVM and the change journal against real storage, and a restored cluster actually starting — is not here either; that belongs with P2's per-backend hold lifecycle jobs, which already have the hosts.

## Fold the block-layer shell-outs into one helper · V2

There are three wrappers around `findmnt` and `lvs` in bestool: the restore path's, the postgresql backup method's, and the strategy detector's. The newest exists partly to get two things right — passing `--` so a volume or logical volume name beginning with a dash cannot be read as an option, and mangling device-mapper names so a hyphenated volume group cannot resolve to a different device — and neither correction reached the two older copies. A `dmsetup message` sent to the wrong pool because of the second one leaves a metadata snapshot reserved on a volume nobody is looking at.

One helper used by backup and restore alike is the fix, so the awkward parts are right everywhere rather than only in the most recently written place. It is its own card because the change lands in the backup capture path — taking btrfs and LVM snapshots, mounting and unmounting them — which is privileged, is only exercised against real storage in CI, and is code a restore card otherwise does not touch at all. It wants a diff and a review of its own rather than riding along in one about restoring.

# Pin kopia's cache directory

## Cause

Every canopy connection uses a fresh transient config and passes no cache directory.
kopia then names the cache `<UserCacheDir>/kopia/<sha256(uniqueID + configPath)[:16]>` (`repo/caching.go`), so each run gets a new directory.
Only `repository disconnect` removes it (`repo/connect.go`), and nothing disconnects: the temp config is dropped and the cache stays for good.
The budget is per directory, so the total is unbounded, and every run starts cold.

## Design

- The cache root is kopia's own cache root for the user whose home we pin: `/var/lib/kopia/.cache/kopia` (else `$HOME/.cache/kopia`), and `%LOCALAPPDATA%\kopia` on Windows.
- A connection passes `--cache-directory <root>/canopy-{push,restore}-<key>`, where the key is a truncated sha256 of bucket and prefix. A hash keeps the bucket off the device in the clear (spec BAK: the device never holds the bucket).
- Push and restore keep separate caches:
  - they budget and split differently, and kopia sweeps to whichever limits the current connection sets, so sharing one would churn it;
  - on Linux, push runs as the kopia user and restore as root, so a shared dir would leave root-owned files the next backup can't write or sweep.
- A restore cache stays warm after use and is removed once unused for a week. Its last use is the mtime of a marker file inside it, touched after each restore-profile connect.
- Every connection sweeps `<root>`:
  - default-named (16-hex) dirs, unless a kopia config in the kopia config dir still points at one (KopiaUI, system install);
  - push dirs for another repository;
  - restore dirs past retention.
- Removal on Linux as root: `chown -R` to the kopia user (daemon has `CAP_CHOWN`), then `rm -rf` as the kopia user via setpriv. The daemon has no DAC write override, so it can't unlink in kopia-owned dirs itself. Sudo path: `sudo -u kopia rm`. Direct and Windows: in-process.
- The volume the budget is measured on is the pinned dir's (nearest existing ancestor).

## Known cost

kopia's `verifyConnect` disconnects on a failed open after the format blob has been read, which removes the pinned cache. The next run is then cold, but nothing is lost.

## Steps

- [x] Spec: BAK local cache (one cache per repository per purpose, persistence, restore retention, sweep)
- [x] kopia crate: move cache sizing into `cache.rs`; add cache dirs, stale selection, removal
- [x] `--cache-directory` on `repository connect`
- [x] bestool `connect_repo`: pin, mark restore use, sweep
- [x] Tests: connect args carry the dir; dir stable across runs/config paths; stale selection
- [x] clippy, fmt, Windows GNU check (bestool's Windows check with `canopy` only fails on the existing `bytes_stream` feature gap, not on this change)
- [x] Checked against kopia 0.23.1: two runs with separate transient configs share the pinned dir and make no hash-named dir; kopia stores `cacheDirectory` relative to the config file

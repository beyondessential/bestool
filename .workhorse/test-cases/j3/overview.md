# Pinned kopia cache

## Automated

- [x] Connect args carry `--cache-directory` with the pinned dir (verifies spec: BAK)
- [x] A repository maps to the same cache dir on every call; push and restore differ; another prefix differs (verifies spec: BAK)
- [x] The cache dir name does not contain the bucket (verifies spec: BAK)
- [x] Sweep removes default-named dirs, push dirs for another repository, and restore dirs past a week; keeps the current push dir, fresh restore dirs, logs, and dirs a live kopia config points at, by absolute or relative path (verifies spec: BAK)
- [x] Unreadable kopia configs keep every default-named dir (verifies spec: BAK)
- [x] A restore cache with no marker ages by its directory's mtime

## Manual, on a Linux host with the daemon

- [ ] After two scheduled backups, `/var/lib/kopia/.cache/kopia` holds one `canopy-push-*` dir and no new 16-hex dirs (verifies spec: BAK)
- [ ] The first backup after upgrade removes the existing 16-hex dirs, as the kopia user, with no permission errors in the journal (verifies spec: BAK)
- [ ] The second backup's `snapshot create` reuses cached metadata (fewer hashed files than a cold run)
- [ ] `bestool canopy restore` leaves a `canopy-restore-*` dir; a backup within the week keeps it; a backup after the week removes it, root-owned files included (verifies spec: BAK)
- [ ] A host with a system kopia repository keeps that repository's cache through a sweep

## Manual, on Windows

- [ ] Backups pin the cache under `%LOCALAPPDATA%\kopia` and sweep the default-named dirs there, leaving KopiaUI's cache alone (verifies spec: BAK)

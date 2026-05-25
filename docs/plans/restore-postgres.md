# `bestool tamanu restore-postgres` — design

## Context

We currently restore Tamanu clones from a kopia snapshot via the ansible
playbook at `~/code/work/ops/ansible/clone-restore.yml`. That playbook
supersedes the manual procedure in `docs/restore-process.txt`. It's been
refined considerably: pre-flight checks, resumability after partial
failures, layout detection, pg-version-mismatch handling, reflink-aware
copying, current-symlink updates.

We want to lift the postgres-restore mechanics out of ansible into a
`bestool tamanu restore-postgres` subcommand so we can:
- Remove the ansible dependency from the restore path (operators don't need
  ansible installed, no inventory plumbing for one-off restores).
- Generalise to **production** restores, not just `clone-*` hosts. The
  playbook hard-gates on the inventory name and hostname containing
  `"clone"`; the bestool command replaces that with an explicit
  confirmation prompt that works for either context.
- Make the operation directly invocable from another host context (e.g.
  triage scripts) without ansible's setup.

## What's in scope

A single command `bestool tamanu restore-postgres` that takes the same
snapshot-selector flags as `bestool kopia restore` and performs the
postgres-data-restore end-to-end through to "postgres is up with the
restored data". Mirrors the `clone-restore.yml` playbook's first two
plays.

What stays out of scope:
- The tamanu-specific config refresh (DB credentials rotation, `local.json5`
  rewrite, reporting users, pg_hba rewrites, postgres tuning) — that's
  `clone-restore.yml`'s third play and depends on ansible roles
  (`postgres-database`, `tamanu-single-install/{reporting-users,local-config}.yml`).
  Operators run those out-of-band, or we lift them later in a separate
  `bestool tamanu refresh-creds` command.
- Starting tamanu — that stays manual via `bestool tamanu start`, same as
  in the playbook.

## Step-by-step mapping from the playbook

| Playbook step | bestool tamanu restore-postgres does |
|---|---|
| Refuse on non-clone hosts | **Different**: doesn't gate on hostname. Instead requires a "type the database name" confirmation prompt, with `--yes` to skip in scripts. The command is meant to work on prod too. |
| Check `kopia repository status` | Reuses `bestool_kopia::build_kopia_command` + a status probe; fails early if not configured. |
| Look up the target snapshot in the listing | Reuses `bestool_kopia::fetch_snapshots` + the same selector flags as `bestool kopia restore`; resolves to a single snapshot before any destructive op. |
| Require recorded size + headroom (size × 1.2) | Same — read from `Snapshot::total_size()` and bail if unrecorded. |
| `bestool tamanu stop` | Reuses the inner functions from `crates/bestool/src/actions/tamanu/stop.rs` (`plan_stop`, `systemctl_stop`, `wait_stopped` — promote from private to module-pub). |
| Check `/var/lib/kopia/restore` + marker for resumability | Same — read `/var/lib/kopia/restore.snapshot-id`, compare against target snapshot ID, reuse the dir if matching (or `--resume` is set). |
| Refuse to overwrite existing restore dir without confirmation | Same; suggests `--resume` in the error. |
| `dropdb tamanu -f` + `VACUUM FULL` | Spawns `sudo -u postgres dropdb tamanu -f` then `psql -c 'VACUUM FULL'`. `--skip-vacuum-full` to skip. |
| `systemctl stop 'postgresql@*'` | Same. |
| Disk space check via `findmnt -n -o AVAIL --bytes -T /var/lib/postgresql` | Uses `sysinfo::Disks` (same approach as `bestool kopia restore`'s pre-flight). Requires `snapshot_size * 1.2` free. |
| Move existing data dir aside with timestamped suffix | Same — `mv main main.replaced-{ISO8601}`. |
| `kopia snapshot restore <id> restore` (async, logs to `/var/lib/kopia/restore.log`) | Same — invoke via `build_kopia_command`; stream/tee output to a log file under `/var/lib/kopia/`. |
| Write `/var/lib/kopia/restore.snapshot-id` marker | Same. |
| Detect layout: `PG_VERSION` at root vs nested `<v>/main` | Same — `one_cluster` if `<staging>/PG_VERSION` exists; `whole_install` if a single `<staging>/<digit+>/main` exists. |
| `apt install postgresql-<v>` if snapshot version differs from installed | Same — invoke `apt-get install -y postgresql-<v>`. `--skip-pg-version-install` to refuse if mismatched. |
| Stop + remove auto-init'd `/var/lib/postgresql/<v>/main` | Same. |
| `cp -r --reflink=auto <staging>/{layout-specific} /var/lib/postgresql/<v>/main` | Same — keeps the staging dir intact for resumability, uses reflink on btrfs/xfs. |
| `chown -R postgres:postgres /var/lib/postgresql`, `chmod 0750 main` | Same. |
| Update `/var/lib/postgresql/current` + `/etc/postgresql/current` symlinks if version changed | Same. `--no-symlink-update` to skip. |
| `systemctl start postgresql@<v>-main`, fall back to `pg_resetwal` on failure | Same. |
| `psql tamanu -tAc 'select count(*) from patients'` | Same — print the count. `--no-verify` to skip. |

## Recommended CLI

```
bestool tamanu restore-postgres [SELECTOR_FLAGS] [OPTIONS]
```

**Selector flags** (mirror `bestool kopia restore`, via the shared
`SnapshotSelectorArgs` flatten):
- `--snapshot ID` — explicit snapshot ID (full or short prefix)
- `--latest` — newest matching snapshot, no prompt (with `--tag`/`--path`)
- `--source-host HOST` — defaults to current hostname
- `--tag KEY:VALUE` (repeatable) — `area:postgres` is the standard tag from
  the backup scripts; not auto-applied (operator may want config-only
  restores)
- `--path SUBSTR`, `--since DURATION`

**Restore options:**
- `--data-dir <PATH>` — override the postgres data dir. Default
  `/var/lib/postgresql/<v>/main` where `<v>` is detected from the snapshot.
- `--staging-dir <PATH>` — kopia restore output dir. Default
  `/var/lib/kopia/restore`.
- `--resume` — reuse an existing staging dir even when the marker doesn't
  match (mirrors the playbook's `resume_existing_restore`).
- `--skip-vacuum-full` — skip `VACUUM FULL` after dropdb (faster on
  spacious hosts).
- `--skip-pg-version-install` — refuse to apt-install a different
  postgres major version. Use when ops controls package installation
  separately.
- `--no-symlink-update` — don't touch `/var/lib/postgresql/current` or
  `/etc/postgresql/current`.
- `--no-verify` — skip the post-restore patient-count check.
- `--dry-run` — print every step, execute none.
- `--yes` / `-y` — skip the destructive-action confirmation prompt.
  Required when stdin isn't a TTY.

## Production safety

The clone-restore playbook gates on hostname containing `"clone"`. This
command needs to work on production restores too, so we replace that gate
with:

1. An interactive **type the database name** confirmation prompt (mirrors
   `dropdb`'s behaviour) before the dropdb step.
2. Non-interactive contexts (no TTY) require `--yes` to proceed.
3. The output stream surfaces the snapshot's source host + tags + age
   prominently *before* the prompt, so the operator can sanity-check
   they're restoring the right snapshot onto the right host.

No `--for-production` flag — the prompt is the gate. Adding a separate
flag would just train operators to type both.

## Cross-cutting concerns

- **Elevation.** Every step needs root (to invoke `become_user`-style
  subprocess switches: postgres for SQL, kopia for snapshot restore). The
  command fails-early with a clear message if not root; doesn't try to
  self-elevate.
- **Failure recovery.** If anything past dropdb fails, the operator can
  rename `main.replaced-{ISO8601}` back to `main` and restart postgres.
  Print that recovery hint at each failure point past the dropdb step.
- **Output.** Stream subcommand stdout/stderr through to the operator.
  Long-running kopia restore tees to a log file (default
  `/var/lib/kopia/restore.log`, surface "tail -f" hint at start).
- **Progress reporting.** Use `tracing` at `info` level for "stopping
  tamanu", "checking disk space", "restoring snapshot kabc12345…",
  "moving aside old data dir", "starting postgres", etc.

## Key files to add/touch

- `crates/bestool/src/actions/tamanu/restore_postgres.rs` — the new command.
- `crates/bestool/src/actions/tamanu.rs` — wire `restore_postgres => RestorePostgres(RestorePostgresArgs)` into the action enum, behind a new `tamanu-restore-postgres` Cargo feature.
- `crates/bestool/Cargo.toml` — add the feature gated on `tamanu-config`,
  `kopia`.

## Existing utilities to reuse

- **Snapshot selection.** `bestool_kopia::SnapshotSelectorArgs::resolve`
  already handles the picker/--latest/--snapshot UX and the TTY gate.
- **Snapshot fetch + size.** `bestool_kopia::fetch_snapshots` +
  `Snapshot::total_size()`.
- **Disk-space check.** Lift `existing_ancestor` + `available_bytes_for`
  from `crates/bestool/src/actions/kopia/restore.rs` into a shared module
  (`bestool-kopia` or a `disk` helper in `bestool`) so both `kopia restore`
  and `tamanu restore-postgres` use the same algorithm.
- **Service lifecycle.** `crates/bestool/src/actions/tamanu/stop.rs` and
  `start.rs` — factor their inner functions (`plan_stop`, `systemctl_stop`,
  `wait_stopped`) into the `lifecycle` module.
- **Tamanu install root + version.** `bestool_tamanu::find_tamanu()` for
  the installed Tamanu version + root (used to source `/etc/tamanu/env`
  for `TAMANU_VERSION` if needed).
- **PG version detection.** `bestool_tamanu::doctor::checks::db_version`
  already runs `SELECT version()` against the running DB — but we want
  the *snapshot's* pg version, which comes from the restored data
  (`<staging>/PG_VERSION` or the numbered subdir). Write fresh helpers.

## Verification

- Unit tests for the layout detection (PG_VERSION at root vs nested under
  `<v>/main`).
- Unit tests for the resume-marker logic (existing dir + matching marker →
  reuse; existing dir + missing/mismatched marker → refuse unless
  `--resume`).
- Unit tests for the confirmation prompt input parser (only the exact DB
  name unlocks).
- Manual end-to-end test on a sacrificial clone host (no replacement for
  this — playbook does it today, command should do it tomorrow):
  - Existing tamanu installed, postgres running, real data
  - `bestool tamanu restore-postgres --latest --source-host prod-host --tag area:postgres`
  - Verify it picks up the snapshot, drops, restores (reusing the staging
    dir on the second run), starts postgres, prints patient count.
- Failure-path manual test: cause a failure between dropdb and restore
  (e.g. invalid `--data-dir`) and confirm the error message tells the
  operator how to recover via `main.replaced-{timestamp}`.
- Resume manual test: run once, kill kopia mid-restore, re-run with
  `--resume` and verify it skips re-extraction.
- Cross-build for `x86_64-pc-windows-gnu`. The command is Linux-only;
  feature-gate accordingly.

## Sequencing

Single PR is feasible, given the scope:

- The implementation is mechanical now that the kopia primitives are
  promoted.
- The lifecycle-helper factoring + reusable disk-space-check extraction
  are small refactors the same PR can do.

If the PR grows too large, split:

1. **Refactor PR:** factor lifecycle helpers + share the disk-space
   helper between `kopia restore` and the new command. No behaviour
   change.
2. **Feature PR:** add `restore-postgres` on top.

The implementation order to follow when writing the command itself,
mirroring the playbook so we can compare results 1:1 during the manual
test:

1. Snapshot resolution + pre-flight (size, disk space) — *before* any
   destructive op, so a misconfigured invocation fails fast.
2. Confirmation prompt.
3. Stop tamanu.
4. Resumability decision (marker check).
5. dropdb + VACUUM FULL.
6. Stop postgres.
7. Move existing data dir aside.
8. kopia restore (or reuse staging).
9. Write marker.
10. Layout detection.
11. pg-version-mismatch handling (apt install + symlink prep).
12. Reflink copy into the data dir.
13. Ownership + permissions.
14. Symlink updates.
15. Start postgres (with pg_resetwal fallback).
16. Verify.

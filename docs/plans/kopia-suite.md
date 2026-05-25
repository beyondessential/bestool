# `bestool kopia` command suite

## Context

We use kopia for postgres backups on a growing share of our servers (about half today). The first PR in this stack landed a `kopia_backup` doctor check that surfaces backup recency. This plan covers operator-facing tooling on top of that: listing relevant backups, restoring them, and integrating with the canopy backup-credentials issuer.

Constraints from the user:
- Top-level namespace is `bestool kopia` (not `bestool tamanu kopia`) — generic kopia operations usable on any server, with tamanu integrations layered separately on top in later PRs.
- The Canopy team is shipping a `/backup-credentials` endpoint (see `~/code/work/canopy/docs/plans/backup-credentials.md`) that issues short-lived AWS creds via the AWS SDK `credential_process` mechanism. Bestool needs to be that helper binary.
- Current manual restore process (see `docs/restore-process.txt`) involves a fair amount of fiddly steps: switch to kopia user, `jq` through the snapshot list to pick one by host+tag, restore, hand-fix paths for the postgres version, fix ownership, sometimes `pg_resetwal`. The headline `restore` command should replace the kopia-mechanical parts; the tamanu-aware dance gets its own wrapper later.

## Recommended command surface (this stack)

Parent group `bestool kopia` with these leaf commands. Each can ship as a separate PR on top of a skeleton PR that adds the parent module + shared helpers.

### `bestool kopia info`
Show repo connection status, last maintenance time, source/host summary. Wraps `kopia repository status`. Cheap to implement and useful as the anchor command for the skeleton PR.

### `bestool kopia list`
List snapshots, defaulting to "what's relevant for this server".
- Default filter: `--source-host` set to current hostname.
- Flags: `--all` (drop host filter), `--source-host HOST`, `--tag KEY:VALUE` (repeatable), `--path SUBSTR`, `--limit N`, `--since DURATION`, `--json`.
- Output: human-readable table by default (id, when, source, size, tags); `--json` for the post-filter kopia output.

### `bestool kopia restore <DESTINATION>`
Restore a snapshot to a destination directory.
- Snapshot selection: `--snapshot ID` for explicit, `--latest` for newest matching, or interactive picker (default).
- `--latest` requires `--tag` or `--path` so the chosen "newest" is unambiguous — a kopia repo holds many kinds of snapshots and an unfiltered "latest" would pick an arbitrary type (postgres data vs config vs other apps).
- Picker uses `dialoguer`; surfaces recent matching snapshots filtered by `--source-host` (defaulting to current hostname), `--tag`/`--path`/`--since`.
- When stdout is not a TTY (e.g. CI, piped to a file), require either `--snapshot` or `--latest` (with `--tag`/`--path`) — fail fast rather than blocking on a prompt nothing will see.
- Wraps `kopia snapshot restore`. No tamanu-specific dance — that lives in a future `bestool tamanu restore-postgres` wrapper that calls this internally.
- Other flags: `--dry-run` (resolve the snapshot ID and print, don't restore), `--overwrite` (allow non-empty destination), `--json`.

The selector flags (`--snapshot`/`--latest`/`--source-host`/`--all`/`--tag`/`--path`/`--since`) live in a shared `SnapshotSelectorArgs` struct in `common.rs`, flattened into both `restore` and `mount` via `#[command(flatten)]`. The resolution logic (including the `--latest` safety gate and TTY check) is one method on that struct.

### `bestool kopia mount <MOUNTPOINT>`
Mount a snapshot read-only via kopia's FUSE backend, for ad-hoc inspection. Snapshot selection is **exactly** the same as `restore` (same `SnapshotSelectorArgs` flatten). Foreground by default (matches kopia's own behaviour); `--background` to detach. On Windows, kopia mount uses WinFsp — best-effort.

### `bestool kopia credentials`
Canopy integration per the backup-credentials plan.
- `--purpose backup|restore` (default `backup`).
- POSTs to canopy `/backup-credentials` using the device's mTLS key (reuses `bestool_canopy::CanopyClient::new(...)` at `crates/canopy/src/client.rs:131`).
- Writes AWS `credential_process` JSON to stdout (Version + AccessKeyId + SecretAccessKey + SessionToken + Expiration). Exits 0 on success, non-zero on failure.
- Per-purpose exit conditions (e.g. 409 from canopy = no backup config for group) map to distinct exit codes so kopia's stderr is informative.

**Canopy plan needs an update.** The current `~/code/work/canopy/docs/plans/backup-credentials.md` specifies `credential_process = bestool backup-credentials` literally; we're naming it `bestool kopia credentials` instead. Update the canopy plan (and any provisioning code that writes the kopia config) to match. The fix is a string change in the plan + wherever the kopia config template lives.

## Out of scope (for later PRs)

- `bestool kopia init` / `bestool kopia connect` — replacement for `ansible/roles/kopia/files/bes-setup-kopia` interactive setup. Substantial scope, not blocking the restore use case.
- `bestool kopia snapshot` — manual backup. The existing shell scripts (`kopia-backup-postgres-{ext4,btrfs}.sh`) are intricate (pg_backup_start/stop, mergerfs overlay, bindfs UID-mapping); reimplementing in Rust deserves its own design.
- `bestool kopia maintenance` — wrap `kopia maintenance run/info`. Already on a systemd timer; nice-to-have.
- `bestool tamanu restore-postgres` — the tamanu-aware wrapper around `bestool kopia restore` that handles stopping services, dropdb, version-path detection, ownership, `pg_resetwal` fallback, inventory bump. Substantial, deserves its own plan.
- Extraction of a `bestool-kopia` library crate. Only worth it if reuse pays off; for now keep everything in `crates/bestool/`.

## Key files to add/touch

- `crates/bestool/src/actions/kopia.rs` — parent module + `KopiaArgs` + dispatch, mirroring `crates/bestool/src/actions/tamanu.rs` (which uses the `subcommands!` macro).
- `crates/bestool/src/actions/kopia/info.rs`
- `crates/bestool/src/actions/kopia/list.rs`
- `crates/bestool/src/actions/kopia/restore.rs`
- `crates/bestool/src/actions/kopia/mount.rs`
- `crates/bestool/src/actions/kopia/credentials.rs`
- `crates/bestool/src/actions/kopia/common.rs` — shared helpers: kopia binary location, snapshot JSON deserialisation, filter logic, `runuser`/sudo elevation. Mirrors how `crates/bestool/src/actions/tamanu/alerts/` keeps shared types alongside the commands.
- `crates/bestool/src/actions.rs` — wire `kopia => Kopia(KopiaArgs)` into the top-level `subcommands!` macro, behind a `kopia` Cargo feature, following the same gating pattern as `tamanu`, `caddy`, etc. (see lines 60–94).
- `crates/bestool/Cargo.toml` — add the `kopia` feature.

## Existing utilities to reuse

- **Kopia binary location** — `crates/tamanu/src/doctor/checks/kopia_backup.rs:locate_windows_kopia_binary()` already handles `%LOCALAPPDATA%\Programs\KopiaUI\…` etc. The first PR that needs it duplicates the logic in `kopia/common.rs`; if a third user appears, lift it into `bestool-tamanu` or a new shared crate.
- **Snapshot JSON shape** — same file's `Snapshot` / `SnapshotSource` deserialisers; expand them (add `id`, `tags`, `size`, `description`).
- **Canopy mTLS client** — `bestool_canopy::CanopyClient::new(version, device_key_pem)` at `crates/canopy/src/client.rs:131`. Already used for posting events and fetching tags; `credentials` adds a `post_backup_credentials` method to it (or calls the underlying mTLS reqwest client directly).

## Cross-cutting concerns

- **Linux: running as the kopia user.** Repo config lives at `/var/lib/kopia/.config/kopia/repository.config`, owned by the `kopia` user. The existing `restore-process.txt` has the operator `sudo -u kopia bash` first. Default behaviour: if Linux + not running as the kopia user + that config path exists, transparently re-exec under `runuser -u kopia -- bestool kopia ...` (or `sudo -u kopia` fallback if not root). Override with `--no-sudo` for cases where the operator has set up their own kopia config. The re-exec preserves all argv and env vars; needs care so terminal modes (for the interactive picker) and stdout streaming (for `credentials`) survive the user switch. (Doctor check intentionally avoids all of this by going through systemctl instead — different problem.)
- **Windows: per-user kopia config.** Just invoke kopia as the current user; no elevation. Mirror the doctor check's `locate_windows_kopia_binary` + `%APPDATA%\kopia\repository.config` path.
- **Output formatting.** Provide `--json` on every command that produces structured data. Default human output uses tab-aligned columns matching `kopia snapshot list`'s style.
- **Feature gating.** New `kopia` Cargo feature in `bestool`. Not included in `__tamanu` since `kopia` is a peer subcommand, not part of the tamanu surface.

## Verification

- Unit tests for snapshot filter logic (mirroring the tests in `crates/tamanu/src/doctor/checks/kopia_backup.rs:tests`).
- Unit tests for the `latest` selector against fixtures.
- Unit tests for `credentials` JSON output shape (per AWS `credential_process` spec).
- Unit tests for the picker's TTY-detection guard (non-TTY without `--latest` or explicit ID = error).
- Manual integration test on a dev box that has kopia configured: `list`, `info`, `mount` (foreground), `restore --dry-run`, then `restore` to a sandbox path with both the interactive picker and `--latest`.
- For `credentials`: stub canopy endpoint or test against a staging canopy.
- Cross-build for `x86_64-pc-windows-gnu` per AGENTS.md.
- `cargo clippy`, `cargo fmt`, full test suite green. `DATABASE_URL=postgresql://localhost/tamanu_meta` for tamanu crate tests.

## Stacking order

Each row is one PR on top of the previous:

1. **Skeleton** — `kopia.rs` parent, `common.rs` shared helpers, `info` command, Cargo feature, top-level wiring.
2. **`list`** — list with the filter flags.
3. **`restore`** — depends on `list`'s selector logic. Includes the interactive picker.
4. **`mount`** — independent of `restore`, can ship in parallel.
5. **`credentials`** — independent; can land before or after the others, lands when the canopy endpoint is ready. Also update `~/code/work/canopy/docs/plans/backup-credentials.md` to reference `bestool kopia credentials` instead of `bestool backup-credentials`.
6. **Plan: `bestool tamanu restore-postgres`** — once the kopia primitives are in place, write a follow-up plan at `docs/plans/restore-postgres.md` that designs the tamanu-aware wrapper. Inputs: `docs/restore-process.txt` (the manual playbook to automate), the new `bestool kopia restore`/`list` primitives, and the postgres version detection logic that already exists in bestool. The plan should cover: service stop/start ordering, dropdb + VACUUM FULL, version-path detection, ownership/permission fix, `pg_resetwal` fallback, optional migration + restart, and how to handle the inventory bump (or whether that stays out-of-band). Pivot straight into implementation once the plan is approved.

That gives the `restore` PR the shortest path to merge — three PRs deep, not blocked on canopy.

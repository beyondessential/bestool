# Not-a-Tamanu-host handling

`/etc/tamanu/env` is always present on Linux Tamanu deployments. When it's
absent, `/etc/tamanu/<version>` directories are meaningless (pre-staged or
leftover config) and the host is "not a Tamanu host": the doctor sweep still
runs and posts to canopy, but every Tamanu-related check reports `skipped`.

Decisions (with user):

- The env-file gate applies to `/etc/tamanu` candidates only;
  package.json-versioned roots (dev checkouts, `/app`, `/opt/bes/tamanu`)
  remain discoverable without it.
- Doctor/alertd on a non-Tamanu host: the sweep runs, Tamanu-dependent checks
  skip, host-level checks still run, and the sweep posts to canopy so the
  fleet sees the host.
- Other `bestool tamanu` subcommands error clearly ("not a Tamanu host").
- X-Version header / payload `tamanuVersion`: `0.0.0` sentinel; canopy
  unchanged.
- Server identity: registration-only. No local UUID minting; with no
  server-id available the canopy push is skipped (existing behaviour).

## bestool-tamanu

- [ ] `try_find_tamanu(root) -> Result<Option<(Version, PathBuf)>>`:
  - explicit `--root`: as today (Err on invalid).
  - Linux, `/etc/tamanu/env` absent: drop all `/etc/tamanu` candidates.
  - env present: active-version signals (podman → DB → env) as already
    implemented; signals that match nothing → fall back to package.json
    roots, else Err (it *is* a Tamanu host, but broken).
  - no candidates left: Ok(None) — not a Tamanu host (any platform).
- [ ] `find_tamanu` wraps it, erroring on None: on Linux without env file,
  "not a Tamanu host: /etc/tamanu/env not found (use --root)"; otherwise
  "no tamanu discovered, use --root". CLI subcommands keep using this.
- [ ] Tests for the gating/selection (factor pure parts).

## bestool-alertd doctor

- [ ] Context split, minimising churn in check modules:
  - `CheckContext` keeps its current fields (Tamanu checks unchanged).
  - New `SweepContext { tamanu: Option<CheckContext>, http_client }`.
  - Registry runners take `SweepContext`. Default `entry!` arm wraps
    Tamanu-dependent checks: when `tamanu` is None, return
    `Check::skip(name, "no Tamanu on this host", ...)` without running.
  - `host` entry arm for host-level checks, which get the `SweepContext`:
    `disk_free` (uses `/` when no Tamanu root), `memory`, `load`, `uptime`,
    `time_sync`, `external_users`, `tailscale`.
  - Everything else is Tamanu-dependent, including `caddy_version`,
    `kopia_backup`, `http_errors`, `tamanu_http` (deployment-stack probes).
- [ ] `perform_sweep` takes `tamanu: Option<SweepTamanu { version, root,
  config, database_url }>` instead of four required params. DB connect and
  `detect_kind` only when present. `server_info::gather` gets `0.0.0` when
  absent. `get_or_create_server_id` unchanged (no DB → file/registration or
  warn-and-skip-push).
- [ ] Sweep test: tamanu=None → Tamanu checks skipped, host checks run,
  overall healthy.

## alertd daemon + CLI

- [ ] `DaemonConfig.pg_pool` / `database_url` become Option (pool is only
  passed through `InternalContext`/`TaskContext`; nothing consumes it on a
  non-Tamanu host). `tamanu_version` string becomes `0.0.0` there.
- [ ] `bestool tamanu alertd run`/`service`: `try_find_tamanu`; on None skip
  config load, pool creation, and DB device-key fetch (registration/file
  paths still apply); `DoctorTask` holds `Option<SweepTamanu>`.
- [ ] `bestool tamanu doctor`: `try_find_tamanu`; on None run the sweep
  tamanu-less (daemon paths unchanged).

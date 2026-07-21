# Idempotent alertd install + canopy register reconcile

Three related asks: stop setup commands erroring when they could reconcile, and
make the Windows service log path consistent with everything else.

## A. Consistent Windows service log path

Today `get_service_log_path()` (`crates/alertd/src/windows_service.rs`) returns
`%ProgramData%\BES\bestool-alertd`; everything else lives under
`%ProgramData%\bestool\…`.

- Change it to `%ProgramData%\bestool\logs` (still a directory, so lloggs keeps
  its daily-rotating JSON + keep-32 behaviour; files become
  `…\bestool\logs\bestool.<ts>.log`).
- Existing services carry the old `--log-file` in their launch args; the normalise
  path (B) rewrites those, so they migrate on the next `install`.
- Old `…\BES\bestool-alertd` logs are left where they are (not migrated/deleted).

## B. `alertd install` detects + normalises instead of bailing (Windows)

`install_service_with_args` currently bails on `ERROR_SERVICE_EXISTS`
("uninstall first"), and `run_diagnostics` prints an "already exists → uninstall"
tip. Make install an idempotent upsert:

- Factor the desired install spec into one place: the `ServiceInfo`
  (exe = `current_exe`, launch args = `["--log-file", <logdir>, "alertd",
  "service"]`, `OWN_PROCESS`, `AutoStart`, LocalSystem), the description, and the
  failure actions.
- `install`:
  - Absent → create as today, then apply description + failure actions + start.
  - Present → open with `QUERY_CONFIG | CHANGE_CONFIG | START | QUERY_STATUS`,
    `change_config` with the desired `ServiceInfo` (idempotent; corrects exe path
    and launch args incl. the new log dir), re-`set_description`, ensure failure
    actions (reuse `is_recovery_configured` / `apply_failure_actions`), then ensure
    it's running (start if stopped) and poll to Running.
  - Log at info when the existing binary path / args differ from desired, so an
    operator sees what was corrected. `ServiceConfig.executable_path` is the whole
    command line as one string, so the drift check compares that string against
    the rebuilt desired command line (best-effort; the upsert applies regardless).
- Keep the only unavoidable hard failure: service "marked for deletion" (needs a
  reboot). Drop the "already exists → uninstall first" framing from
  `run_diagnostics`.
- `create_service` already requests `CHANGE_CONFIG | START`; the reconcile reopen
  requests `QUERY_STATUS` too.

This extends the existing self-heal precedent (`configure_recovery` already runs
on every `alertd service` start). Scope stays on the `install` command, not
startup.

## C. `canopy register` auto-reconciles a prior registration (cross-platform)

`register` (`crates/bestool/src/actions/canopy/register.rs`) bails upfront if any
registration exists, telling the user to `unregister` first. `unregister`
(`unregister.rs`) is local-only — canopy has no device-delete endpoint, so remote
cleanup of the old device id isn't possible either way (a pre-existing limitation,
not something this change regresses).

- Drop the upfront "already registered" bail.
- Before minting the new identity, load and stash the existing `Registration` (so
  a later step could act on the old ids; today only local cleanup is possible).
- Run the enrolment (mint identity → begin → complete) exactly as now.
- **Only on `complete()` success** (the signal register already trusts; on the
  mTLS path it exercised the new device cert): `store_in` the new registration,
  then run the same local teardown `unregister` does — legacy `/etc/tamanu`
  device-key/server-id, `tags.json` (both dirs), and the `local_system_facts`
  rows — so the daemon can't re-seed the stale identity.
- Factor that local teardown into a shared helper called by both `register`
  (post-success) and `unregister`.
- If enrolment fails before store, the old registration is left untouched — a bad
  ticket never strands the host. No upfront destruction.

No post-enrol verification call is added; `complete()` success is the confirmation.

## Non-goals

- Remote deregister of the old device id (no canopy API exists).
- Migrating/deleting old `…\BES\bestool-alertd` logs.
- Normalising the service on every startup (install-time only, as asked).
- Reworking how `--log-file` / lloggs rotation works.

## Unit tests to write

- The desired-command-line builder and the drift comparison (pure string build).
- The shared local-teardown helper's target path list.

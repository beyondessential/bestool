# Add healthcheck for missing device_id in registration

## Design notes

- The healthcheck lives as a new host-level check, `canopy_registration`, in the alertd doctor registry (`crates/alertd/src/doctor/checks/`). It runs regardless of Tamanu install or backups being enabled, and is on the wire so Canopy sees incomplete enrolments. It reads the registration via `bestool_canopy::registration::load` and grades statically (no network probe).
- Grading is captured in the `REG` spec (`.workhorse/specs/canopy/registration.md`).

## Investigation finding: the device_id hard-fail is not in the client

The client-side backup code already handles a missing `device_id` cleanly: `backup_after_start` reads it as an `Option` and `assemble_tags` only inserts the `canopy-device` tag when it is present (crates/bestool/src/actions/canopy/backup.rs). A missing `device_id` therefore drops one tag; it does not error locally.

So backups failing on every host with no `device_id` points at **Canopy server-side** rejecting a snapshot/report that lacks the `canopy-device` tag. That is a separate codebase (not in this repo) and a separate fix from this healthcheck. This card's job is the pre-flight signal; confirming/repairing the server-side requirement should be tracked separately.

## Build steps

- [ ] Add `canopy_registration` check module and register it (host-level, on-wire).
- [ ] Grade per the REG spec: no record / no server id / no device id -> fail; no device key -> warn; server id + device id + device key present -> pass (api_url optional).
- [ ] Unit-test each state -> outcome, mirroring the check test style in the doctor checks modules.

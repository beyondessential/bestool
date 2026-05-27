# Version mismatch detection in `tamanu status` / `doctor`

## Problem

`tamanu status` reports every expected-Up service as OK as long as the
right number of instances are running. It says nothing about *what
version* those instances are on. So if an upgrade has been partially
rolled out — `/etc/tamanu/env` already bumped to `v2.11.0` but some
containers still on `v2.10.0` (forgotten restart, blue/green frontend
swap interrupted, etc.) — the status check is misleadingly green.

Likewise `tamanu doctor`, which is the "ops alignment" surface: if it
doesn't notice version drift, neither will anyone else.

## Goals

- Surface, per running instance, the actual version vs the expected
  version.
- Treat a mismatch as a non-OK outcome in both `status` and `doctor`,
  not just a warning.
- Handle `TAMANU_FRONTEND_VERSION` correctly — frontend quadlets use
  that variable; everything else uses `TAMANU_VERSION`.

## Non-goals

- Auto-restarting drifted containers. The point is to *flag* the
  state; remediation is a separate ops action (the user might be
  mid-upgrade and the drift is expected for a few seconds).
- Pinning a "minimum acceptable version" — we just compare against the
  configured expectation.

## Sources of truth

### Expected version

Linux container deployments:
- `/etc/tamanu/env` is the canonical source. It is set by the ansible
  install role and read by the quadlet `EnvironmentFile` directive.
- Two relevant keys:
  - `TAMANU_VERSION` — applies to API / tasks / sync / fhir-* /
    patient-portal containers.
  - `TAMANU_FRONTEND_VERSION` — applies to `tamanu-frontend@*`
    containers only. Defaults to `TAMANU_VERSION` if missing.
- The corresponding `*_DOMAIN` keys are derived from the same values
  and don't need their own parsing.

Windows / pm2 deployments:
- Each pm2 process runs out of the install root that `find_tamanu`
  already returns. There is one version per host (the one in
  `package.json` at the discovered root). No frontend-specific
  override exists.

So the expected-version probe returns:
- per-service expected version (string), accounting for the
  frontend-specific override on Linux.

### Actual version

Linux:
- Each running container's image tag is the source. For a service
  `tamanu-X@N.service` we want the image of the container backed by
  that unit:

      podman ps \
          --filter label=PODMAN_SYSTEMD_UNIT=tamanu-X@N.service \
          --format '{{.Image}}'

  yields e.g. `ghcr.io/beyondessential/tamanu-central:v2.10.0`. We
  split on `:` and take the tag.
- Non-running expected-Up instances: no actual version available
  (the status row is already non-OK on "stopped"; we don't need to
  invent an actual version for it).
- One `podman ps` call (no filter) is cheaper than one-per-unit. We do
  a single bulk listing and key by `PODMAN_SYSTEMD_UNIT` label.

Windows / pm2:
- All processes share the install root; the actual version is the one
  `find_tamanu` returns. Same value for every instance.

## API shape

New module: `crates/bestool/src/actions/tamanu/versions.rs`.

Exposes:

```rust
/// Versions expected for the deployment, broken out by which env var
/// drives each service kind.
pub struct ExpectedVersions {
    pub tamanu: Option<String>,
    pub frontend: Option<String>,
}

/// Best-effort lookup. Linux: parses /etc/tamanu/env. Windows: reads
/// from find_tamanu's result. Both fall back to None on error and let
/// the caller decide what to do.
pub fn expected_versions(supervisor: Supervisor, root: &Path) -> ExpectedVersions;

/// Per-unit running version. Keyed by `unit()` (e.g.
/// `tamanu-central-api@1.service`).
pub fn running_versions(supervisor: Supervisor) -> Result<HashMap<String, String>>;

/// Helper: what version SHOULD `name` be on? Picks frontend when the
/// expectation name starts with `tamanu-frontend`, else tamanu.
pub fn expected_for(versions: &ExpectedVersions, expectation_name: &str) -> Option<&str>;
```

Why split expected/actual into two functions instead of one
"resolve_status" that does both?

- They have different failure modes — expected can be read from a
  file even when podman is down; running needs podman.
- Tests for the parsing logic stay pure (no podman dep).
- The caller (`status` and `doctor`) already iterates expectation
  groups; threading two side-tables through is cleaner than reshaping
  the discovery output.

## Integration: status

`crates/bestool/src/actions/tamanu/status.rs`

- Add a `Version` column between Actual and Reason.
- For each instance row, render:
  - `vX.Y.Z` in green when actual == expected
  - `vX.Y.Z (expected vA.B.C)` in red when mismatched
  - dash / empty when no actual (instance not running)
- Mismatch counts as `any_short`, so the command exits non-zero like
  for a stopped service.
- JSON output: add `version_actual`, `version_expected`, and a
  `version_status` of `"match" | "mismatch" | "unknown"` to each
  `InstanceReport`. Lift any_short to include version_mismatch.
- Per-instance, not per-expectation: a frontend swap can leave the
  `@a` slot on the new tag and `@b` still on the old.

## Integration: doctor

`crates/bestool/src/actions/tamanu/doctor.rs`

- Add a new check (alongside the existing service / DB / port checks)
  that emits one row per drifted instance.
- Format consistent with other doctor rows.
- A mismatch is a `Bad` (not `Warning`) since it implies a stale
  deployment.

## Edge cases

- `/etc/tamanu/env` missing: expected = None for every service. We do
  NOT count this as a mismatch — render a dash in the Version column
  and log at debug. (Without the env file there's no installed-expectation
  to compare against; flagging it as a service mismatch would be wrong.)
  Doctor's existing "deployment looks weird" checks already cover the
  case of a half-set-up host.
- `TAMANU_FRONTEND_VERSION` missing but `TAMANU_VERSION` present: this
  is the common case for older deployments where frontend follows
  the API. Treat the frontend expectation as `TAMANU_VERSION`.
- Image tag missing the `:tag` part (raw image ID, etc.): render
  `unknown` and treat as mismatch when expected is set.
- Container running but not labelled with PODMAN_SYSTEMD_UNIT (e.g.
  hand-started for debugging): not associated with any expectation,
  ignored.

## Tests

- `parse_env_file` — covering `TAMANU_VERSION` only, both set, only
  frontend, malformed, comments, leading/trailing whitespace.
- `expected_for` — frontend service picks frontend version, others
  pick tamanu, fallback to tamanu when frontend missing.
- `parse_image_tag` — extracts tag from `repo:tag`, returns None for
  unqualified images.
- Status renderer: snapshot test for a mixed group (one match, one
  mismatch, one unknown).
- Doctor probe: similar snapshot.

The `podman ps` and `find_tamanu` calls themselves stay
integration-test territory (not in the unit suite — they shell out).

## Commit shape

Implement as a stack:

1. `feat(tamanu): probe expected and running tamanu versions` — new
   `versions.rs` module + tests, no UI impact yet.
2. `feat(tamanu): status reports per-instance version drift` — status
   integration.
3. `feat(tamanu): doctor flags tamanu version drift` — doctor
   integration.

Each commit compiles + tests cleanly. Final `unplan:` drops this file.

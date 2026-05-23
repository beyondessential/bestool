# `tamanu` lifecycle subcommands

Four new subcommands — `start`, `stop`, `restart`, `status` — built on top
of the `services::expected()` model (from #336) and the `pm2` helper.
They replace what #313 was trying to do, re-grounded on the post-#336
primitives. There's enough reusable shape in #313's `reload.rs`
(systemd/pm2 restart, HTTP probe, container IP lookup) to lift directly,
while leaving behind the bespoke service discovery and kind-detection
logic.

A follow-up PR (see `tamanu-logs-multiname.md`) reshapes `tamanu logs`
to share the same matcher surface. Stacked on top of this one so the
matcher infrastructure lands first.

Linear: TAM-6782 (carry forward from #313).

## Goals

- `tamanu start [NAMES...]` — idempotent "make sure everything that
  should be up is up". Starts any expected `Up` service that's not
  currently running. No-op for already-running services.
- `tamanu stop [NAMES...]` — bring everything down cleanly. For
  maintenance windows / host work.
- `tamanu restart [NAMES...]` — rolling restart of everything currently
  up. Critical services restart one-at-a-time with a per-instance
  readiness probe; background services restart in bulk.
- `tamanu status [NAMES...]` — render the current discovery state vs
  expectations. No sudo, no HTTP probes. The lightweight cousin of
  `tamanu doctor`.

All four honour the same `Expectation` set the doctor uses, so the
authoritative list of "what should be running" lives in one place.

## The NAMES positional argument

All lifecycle subcommands take a variadic positional `NAMES...` — zero
or more substring patterns matched against expectation names. An
expectation matches if **any** of the supplied names is a substring of
its name (union semantics).

- `bestool tamanu restart` — all expectations
- `bestool tamanu restart api` — anything containing "api"
- `bestool tamanu restart api fhir` — anything containing "api" or "fhir"
- `bestool tamanu status` — everything

If any supplied name matches zero expectations, bail listing it as
unknown alongside the available names. (A non-matching name in a
multi-name invocation is almost certainly a typo; better to surface than
silently drop.)

Empty `NAMES` = "all expectations".

The matcher lives in `lifecycle.rs::match_names(expectations, &[&str])`
and is shared across all four commands. The follow-up logs PR will
reuse it.

## Critical vs background

A new field on `Expectation`:

```rust
pub enum Criticality {
    /// Must always have at least one instance up. Rolling restart only:
    /// restart one instance, probe it ready, then move to the next.
    /// Currently: API and frontend.
    Critical,
    /// No availability constraint. Restart can be done in bulk (all at
    /// once or with minimal pacing). Currently: tasks, sync, fhir-*.
    Background,
}
```

Set per-expectation in `services::expected()`:
- `tamanu-*-api` / `tamanu-api` → Critical
- `tamanu-frontend` → Critical
- Everything else expected `Up` → Background

`ExpectedState::Down` services don't carry a criticality (they shouldn't
run at all). Add a unit test fixing the criticality matrix per
kind/supervisor.

Criticality only affects `restart`'s ordering. `start`, `stop`, and
`status` ignore it.

## `tamanu status [NAMES...]`

Just discovery, rendered. For each matching expectation:

- expectation name + state (Up/Down) + criticality
- discovered instances (count + identifiers: systemd `@instance`, pm2
  `pm_id`)
- per-instance running/active flag

No sudo, no network. Exit 0 if every Up expectation has at least
`min_count` running instances; non-zero if anything is short.

Output: colour-coded human render by default; `--json` for the wire
shape (same convention as `doctor`).

## `tamanu start [NAMES...]`

Pseudo-code:

```
expectations = services::expected(...) ∩ NAME matcher
for each Up expectation:
    discovered = enumerate live instances via systemd/pm2
    missing = expected min_count - discovered
    if missing > 0:
        queue: each missing instance
issue one start call covering every queued instance
wait for all queued instances to be active
```

Discovery primitives already exist:
- systemd: parse `systemctl list-units --type=service --output=json
  tamanu-*` (mirrors what doctor's `tamanu_service` check does).
- pm2: `pm2::list()` → already returns `running` + `pm_id`.

Starting (single call where possible):
- systemd: collect every missing unit (`<base>@<instance>.service` for
  templates, `<base>.service` for singletons; `NumericAtLeast(n)` fills
  `@1..@n`) and issue `systemctl start u1 u2 ... uN` in one invocation.
- pm2: same — collect every stopped-but-known process and call `pm2 start
  <name1> <name2> ... <nameN>` once. If an expected process isn't known
  to pm2 at all, bail with a clear error pointing at the ops setup
  playbook (we don't read the ecosystem file ourselves).

Exit behaviour: 0 on full success; non-zero if any expected service is
still not up after start attempts.

## `tamanu stop [NAMES...]`

Mirror of `start`:

```
expectations = services::expected(...) ∩ NAME matcher
discovered = enumerate live instances grouped by expectation
issue one stop call covering every running instance
wait for all of them to be inactive
```

- systemd: `systemctl stop u1 u2 ... uN` in one invocation.
- pm2: `pm2 stop <name1> <name2> ... <nameN>`.

No ordering distinction between critical and background — once you've
decided to bring things down, the supervisor handles the synchronous
stop and there's no rolling concern. Caddy is not touched (its config
keeps the old upstream addresses; the upstreams just become
unreachable, which is the operator's intent here).

Exit 0 if every targeted instance is no longer running.

## `tamanu restart [NAMES...]`

Pseudo-code:

```
expectations = services::expected(...) where state == Up, ∩ NAME matcher
discovered = enumerate live instances grouped by expectation

# Background first — single restart call covering every background instance.
systemd: `systemctl restart <unit1> <unit2> ... <unitN>` for every
    background instance across all background expectations.
pm2: `pm2 restart <name1> <name2> ... <nameN>` (pm2 takes multiple
    targets in one invocation).

# Critical: rolling, one instance at a time.
for each Critical expectation:
    for each discovered instance (ordered deterministically):
        restart that one instance
        wait_active (systemd) / wait_online (pm2)
        probe http ready (lift from #313's reload.rs)
        reload caddy + flush systemd-resolved (systemd only)
        sleep cooldown
```

Flags:
- `--check-url <URL>` — optional external HTTP probe hit between
  critical-service restart batches. Retained from #313.
- `--cooldown <DURATION>` — default 30s, parsed via jiff (per memory).
  Sleep between each critical instance roll.
- `--no-probe-http` — skip the per-instance HTTP probe. Retained from
  #313.

For systemd, `reload_caddy()` + `flush systemd-resolved` runs after
*every* critical instance restart — needed so caddy picks up the new
netavark IP of the restarted podman container, and must happen between
every instance so the probe against the next instance sees a
freshly-routed caddy. Lift the helper from #313.

For containerised systemd units, `container_ip_for_unit()` resolves the
netavark IP via `podman inspect`; probe `http://<ip>:3000/`. For pm2,
use the `PORT` env var (pm2 sets via `increment_var`) and probe
`http://127.0.0.1:<PORT>/`. Both already implemented in #313 — lift
verbatim with the new Context signature.

### Ordering

Background → Critical: restarting tasks/sync/fhir mid-restart of the API
would briefly drop their connection to the API anyway; doing them first
means by the time we're rolling APIs, the background processes are
already on the new code and ready to talk to whichever API instance is
up.

Within a Critical expectation, restart instances in a deterministic
order (by `@instance` suffix for systemd templates, by `pm_id` for pm2)
so operators see a predictable sequence.

### Self-elevation

#313 added `ensure_root_or_reexec()`: if invoked without root on a
systemd host, re-exec the same args via sudo. Lift verbatim for
`start`, `stop`, `restart` (anything that mutates state); skip it for
`status`.

## Code organisation

```
crates/bestool/src/actions/tamanu/
    services.rs            # add Criticality enum + field on Expectation
    lifecycle.rs           # new: shared primitives
        - discover(supervisor) -> Vec<(Expectation, Vec<Instance>)>
        - match_names(expectations, &[&str]) -> filtered set
        - start_instances(supervisor, &[Instance])
        - stop_instances(supervisor, &[Instance])
        - restart_instance(supervisor, &Instance)
        - wait_active(...) / wait_online(...)
        - probe_http(...)
        - reload_caddy() / flush_resolved()
        - ensure_root_or_reexec()
    start.rs               # tamanu start [NAMES...]
    stop.rs                # tamanu stop [NAMES...]
    restart.rs             # tamanu restart [NAMES...]
    status.rs              # tamanu status [NAMES...]
```

The four lifecycle entry points are thin orchestration layers over the
shared `lifecycle` module. `logs.rs` is untouched in this PR — the
follow-up logs PR (see `tamanu-logs-multiname.md`) reshapes it to use
`lifecycle::match_names`.

## Out of scope for the first cut

- pm2 "expected service not registered with pm2 yet" recovery in `start`.
  Bail with a clear error pointing at the ops setup playbook for
  first-time registration; we don't try to read the ecosystem file
  ourselves.
- Restart of caddy itself (#313 only `caddy reload`d as a side-effect;
  we keep that behaviour). A `tamanu restart caddy` could come later
  but isn't part of this plan.
- `enable` / `disable` (boot enablement) — supervisor-specific and
  ops-install-script territory.

## Testing

- Unit tests in `services.rs` fixing the criticality matrix (alongside
  existing `expected()` tests).
- Unit tests on `lifecycle.rs` `match_names` substring matching against
  the expectation set: empty input = everything, multi-name union,
  zero-match-name bail path.
- Unit tests on `lifecycle.rs` restart ordering: given a fake discovery
  result, the call sequence Background-then-Critical is correct.
- Integration tests against a real systemd or pm2 setup are out of
  scope; rely on the doctor's existing test coverage of the discovery
  code paths and on manual smoke-testing on a staging server.

## PR

Title: `feat(tamanu): TAM-6782: lifecycle subcommands (start/stop/restart/status)`
(carry the Linear ticket forward from #313).

## Stacking

Branch off `main` directly (#341 has merged, so the new Context API is
on main). The follow-up logs PR (`tamanu-logs-multiname.md`) stacks on
top of this one because it consumes `lifecycle::match_names`.

Treat #313 as superseded once this lands and close it.

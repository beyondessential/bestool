# Self-healing healthchecks + canopy registration identity recovery

Implements [CHK](../../specs/tamanu/healthchecks.md#self-healing) (generic heal framework) and [REG](../../specs/canopy/registration.md#recovering-a-missing-identity) (canopy_registration recovers a missing server/device id from Canopy's `GET /servers/self`).

## Design

A check may declare an optional heal action alongside its `run`. The sweep runner, in the daemon only, spawns that action detached when the check's outcome is not a pass, backoff allows, and no attempt for that check is already in flight. Heal outcomes never touch the reported status; a success shows up on a later sweep.

Key mechanisms:

- **Heal enablement is a sweep-level flag.** The daemon enables healing; the one-shot `doctor` CLI never does, so running it by hand has no side effects. Generic — not canopy-specific.
- **Per-check backoff + in-flight guard** live in a process-global registry keyed by check name (matches the existing `probe_cache` / registration `CACHE` patterns). A min interval, exponential backoff to a ceiling, and a single-attempt-in-flight guarantee.
- **Detached execution.** The heal runs in a spawned task so the sweep and its Canopy status-post never wait on it.
- **The shared `CanopyClient` reaches the healer via `SweepContext`.** `SweepContext` moves to a `bon` builder (already a workspace dep) so adding this field — and future ones — doesn't churn every construction site. The daemon passes its warm client; the CLI passes none.
- **Registration cache must be updatable.** `registration::load()` memoises the first read in a `OnceCell` that's never invalidated; a heal that writes to disk but leaves the daemon running would keep reading the stale partial. `store()` refreshes the in-process cache so the healed registration is seen on the next sweep without a restart.
- **`/servers/self` is served by the generated client.** Adding the path + `SelfResponse` schema to the OpenAPI snapshot makes `client.servers_self()` exist; it routes `/public/servers/self` over tailscale (device resolved from tailnet identity) and `/servers/self` over mTLS. Sourced from canopy PR #396 pending its merge; reconciled on the next snapshot sync.

## Checklist

- [ ] **Registration cache is updatable.** Replace the `OnceCell` with a form `store()` can refresh; `store()`/`store` at the default dir updates the in-process cache. `load_from(dir)` still bypasses the global cache. Test: an in-process `store()` is visible to the next `load()`.
- [ ] **OpenAPI snapshot: add `GET /servers/self` + `SelfResponse`.** Exact additions from canopy PR #396. Confirm `servers_self()` and the `SelfResponse` type generate and compile.
- [ ] **`SweepContext` → `bon` builder + `canopy` + `enable_heal`.** Add `bon` to alertd's Cargo. Add `canopy: Option<Arc<CanopyClient>>` and `enable_heal: bool` (default false). Migrate construction sites (sweep.rs, checks.rs test helpers). `perform_sweep` takes the canopy client + heal flag; daemon passes its client and enables healing, CLI/doctor and the sweep test pass none/false.
- [ ] **Heal framework.** `HealOutcome` (Healed / Deferred / Failed). `CheckEntry.heal` optional hook + `entry!` variants to attach one. Backoff/in-flight registry in `doctor/heal.rs` with `try_begin(name)` / `finish(name, outcome)`. Sweep runner spawns the healer detached when enabled + non-pass + `try_begin`. Tests: backoff advance/ceiling/reset, in-flight guard, gate on non-pass.
- [ ] **`canopy_registration` healer.** `heal(ctx)`: needs `ctx.canopy`; calls `servers_self()`; fills a missing `server_id`/`device_id` (leaves `device_key`/`api_url`), `store()`s, returns Healed/Deferred. Maps `401`/`409`/`412` → Deferred (logged), network/other → Failed. Register the hook. Test the merge (pure helper) and the error mapping.
- [ ] **fmt + clippy + tests.**
- [ ] **Test-cases file** at `.workhorse/test-cases/alertd-check-self-heal/overview.md`.

## Open dependency

Canopy PR #396 (`GET /servers/self`) is not merged. The snapshot addition is sourced from it; if the endpoint changes before merge, re-sync the snapshot.

# Move the alertd daemon into the bestool binary

This card relocates code and changes no behaviour, so the verification is that
every build configuration still compiles, the existing tests still pass on
whichever side of the seam they landed, and the checks crate no longer carries
the daemon's dependencies.

## Build configurations

- [x] The workspace builds on Linux with default features
- [x] The workspace builds for a Windows GNU target with default features
- [x] `bestool` builds with the daemon feature off (defaults minus `alertd` and
      `alertd-tamanu`), so `bestool tamanu doctor` does not require the daemon
- [x] `bestool` builds with only the daemon feature on
      (`--no-default-features --features alertd`), so a host with no Tamanu
      deployment can still run the daemon
- [x] `cargo clippy --workspace --all-targets` is clean of new warnings
- [x] `cargo fmt` leaves the tree unchanged

## Dependency separation

- [x] `bestool-alertd` no longer depends on `axum`, `tower-http` 0.7,
      `tokio-stream`, `sd-notify`, `win32job`, or `windows-service`
- [x] `bestool-alertd` no longer depends on `bestool-kopia`, which it had
      stopped using
- [x] The daemon's dependencies are reachable only through `bestool`'s `alertd`
      feature

## Tests move with their code, and none are lost

- [x] `bestool-alertd`'s remaining tests all pass
- [x] The tests that moved pass in `bestool`
- [x] The pass/fail totals across both crates match the totals before the move,
      with the same two postgres-dependent tests failing in an environment with
      no reachable database

## The daemon's connection pool

- [x] The sweep takes its connection from a pool, and only from a pool — there
      is no `connect_one` fallback left in the sweep path
- [x] `bestool tamanu doctor` builds its own pool, so it runs the same path as
      the daemon
- [x] A failed pool acquire leaves DB-dependent checks skipping
- [x] The endpoint tests need no database, because the state they build no
      longer carries a pool
- [ ] With postgres down at daemon start, the daemon still starts, and the
      doctor task builds its pool on a later tick once postgres returns
- [ ] An in-place upgrade that changes the database URL rebuilds the pool
      without a daemon restart
- [ ] With postgres going down while the daemon runs, `db_connect` still reports
      it — the check opens its own connection and never goes through the pool
- [ ] A sweep returns its pooled connection afterwards, so the pool does not
      leak a connection per sweep

## Behaviour preserved across the seam

- [x] The outbound User-Agent stays `bestool-alertd/<alertd version>` rather
      than becoming the bestool binary's version
- [ ] A running daemon still answers `/status`, `/health`, `/metrics`, and
      `/tasks/{task}/{endpoint}` — needs a host with a reachable database
- [ ] `bestool alertd status`, `reload`, and `restart` still reach a running
      daemon
- [ ] On Windows, `bestool alertd install` registers the `bestool-alertd`
      service and it starts
- [ ] The daemon still starts when postgres is down, so the `db_connect` check
      can report it

## CI

- [x] The postgres-less namespace job runs the whole `bestool-alertd` lib test
      binary, rather than filtering on a module path the flatten removed
- [x] A job builds the `alertd` feature on its own, which nothing in CI covered

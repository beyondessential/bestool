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

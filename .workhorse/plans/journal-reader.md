# Replace `journalctl` subprocess for log streaming (draft — undecided)

## Context

`crates/bestool/src/actions/tamanu/logs.rs:379` (`run_journalctl`) shells out to `journalctl` to tail tamanu service logs. It uses `--output=cat` to get raw MESSAGE text, pipes stdout, and applies optional client-side regex filtering for the inverted-match (`-v`) case.

This works fine today. The question is whether to keep it as-is, structure the subprocess output better, or move to a library — now that the systemctl side is on `zbus_systemd`, this is the only remaining subprocess for systemd interaction.

bestool is GPL-3.0-or-later, so LGPL deps are fine; the decision is on engineering merits.

## Options

### (A) `systemd::journal` (rust-systemd, libsystemd FFI)

The mature reader: `add_match` (`_SYSTEMD_UNIT=tamanu-frontend@a.service`, one per unit), `seek_tail` + `previous` for backfill, `wait` + `next` for follow. Sync API; wrap in `spawn_blocking`.

**Wins**: no subprocess, real journald-side filtering, integrated with our event loop (the journal fd is select/epoll-friendly), full record metadata access (PRIORITY, MESSAGE_ID, _PID, _CMDLINE) for richer output.

**Costs**: build depends on `libsystemd-dev` (pkg-config). Cross-compiling from non-Linux dev boxes gets harder. Static-musl release builds need libsystemd's musl story sorted. `cargo install bestool` and binstall paths may need attention.

**Verdict**: the right answer if we commit to "no subprocesses for systemd", but the build-system change isn't free.

### (B) `journalctl --output=json-seq` + serde

Keep the subprocess; replace `--output=cat` parsing with structured JSON records. Each line is `<RS>{"_SYSTEMD_UNIT": "...", "MESSAGE": "...", "PRIORITY": "3", "__REALTIME_TIMESTAMP": "...", ...}<LF>` — parse with `serde_json`.

**Wins**: enables richer formatting (show unit when tailing multiple, colour by priority, jiff timestamps) without touching the build system.

**Costs**: same subprocess fragility as today; not a real architectural change.

**Verdict**: cheap upgrade if we want the richer output. No-op if we don't.

### (C) Status quo

Leave `run_journalctl` alone. Subprocess + `--output=cat` + line-by-line.

**Verdict**: fine. The code works.

## Criteria for deciding

- Do we want richer log formatting (per-unit prefix, priority colouring)? → (B) or (A)
- Do we want zero subprocesses for systemd? → (A)
- Are we willing to take on a libsystemd C link in the bestool build? → (A) yes; (B/C) no
- Is there a near-term feature that needs structured record metadata (e.g. alerting on PRIORITY=err)? → (A) or (B)

Discarded: `systemd-journal-reader` (pre-1.0, no live follow, format-version risk) and `journald-query` (very new, undocumented).

## Not decided yet

This plan stays in the repo until we pick one. If we never decide, that's fine — (C) is a stable terminus.

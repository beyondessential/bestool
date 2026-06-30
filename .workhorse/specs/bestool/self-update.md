---
id: UPD
---

# Self-update

`bestool self-update` replaces the running bestool binary in place with the latest published release.
On Windows the alert daemon also keeps the binary current on its own, so a host running the service stays up to date without anyone invoking the command.

## Published artifacts

Each release publishes, per target, a downloadable release artifact carrying the bestool binary, alongside a single file naming the latest released version.
A detached signature sits next to each artifact, at the artifact's URL with a `.minisig` suffix.

## Signature verification

Every released artifact is signed with a private key held only by the release pipeline.
bestool embeds the corresponding public key as a trust anchor.
Before installing — whether the update is invoked manually or performed by the daemon — bestool fetches the detached signature for the downloaded artifact and verifies the artifact against the embedded public key, and only then unpacks and swaps in the binary it contains.
An artifact whose signature is missing or does not verify is never installed; the update fails instead.

The same public key is recorded in the binstall metadata, and the signature covers the same artifact binstall downloads, so `cargo binstall` verifies it on its installs too.

## Manual update

`bestool self-update` resolves the latest published version, and unless a specific version or a forced reinstall is requested, does nothing when already on that version.
Otherwise it downloads the target artifact, verifies its signature, and replaces the running binary.

On a package-managed install (the Linux deb) the command refuses to act unless forced, directing the operator to the package manager.

## Delegation to the running daemon

On Windows, when the alert service is running, `bestool self-update` asks the daemon to perform the update rather than swapping the binary itself.
The daemon owns the download, verification, binary replacement, and service restart in one place.
When the service is not running, the command updates the binary directly.

The daemon exposes this through an endpoint that reports whether an update is warranted — and to which version — then carries out the download, install, and restart.
Because the restart drops the connection, the reply is the decision, not a live progress stream.
The command reaches the daemon over whichever loopback address answers, since the daemon binds only the first it can.

## Automatic update by the daemon

The Windows alert daemon checks once a day whether a newer version has been published, at a time staggered across hosts so a fleet neither fetches nor restarts in lockstep.
When a strictly newer version is available it downloads and verifies it, replaces its own binary, and restarts the service so the new binary takes effect.

The daemon does not repeatedly attempt a version that has already failed to install.
Checking and updating never block or stop the daemon's other duties: a failed update is logged and retried on a later check, and the daemon keeps running the current binary in the meantime.

This behaviour is Windows-only.
On Linux the binary is kept current by the package manager, and the hardened service deployment cannot write its own binary in any case.

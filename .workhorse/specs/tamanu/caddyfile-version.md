---
id: CHK-CFV
---

# Caddyfile version healthcheck

The Tamanu doctor runs a check that verifies the Caddyfile on a Windows Tamanu server declares a supported configuration version. Tamanu ships its Caddyfile with a version marker on the first line, and newer Tamanu releases depend on newer Caddyfile revisions; this check surfaces servers still running an outdated Caddyfile so it can be upgraded before it breaks.

It is one of the healthchecks described by `tamanu/healthchecks.md` and follows the shared outcome model in `tamanu/doctor.md` (pass, skip, warning, fail, broken).

## Applicability

The check only applies to a Windows host that has a Tamanu deployment.

- It skips on any non-Windows host.
- It skips on a Windows host with no Tamanu deployment.
- It skips when caddy is not present — when no Caddyfile is found on disk at the standard Windows location.

A skip carries a reason naming which precondition was not met.

## Version marker

Tamanu's Caddyfile declares its version on the literal first line, in the exact form `# tamanu caddyfile v<N>`, where `<N>` is the version number.

- The marker must be the first line of the file. Trailing carriage-return and whitespace are ignored so the CRLF line endings of a Windows file still match.
- `<N>` is read as an integer, so the marker keeps working past a single digit.
- When the Caddyfile is present but its first line is not a valid marker in this form, the check fails, reporting that the Caddyfile version is missing or unreadable.

## Outcomes

For a Windows Tamanu server with a readable Caddyfile:

- [ ] A Caddyfile whose marker version is 9 or greater passes.
- [ ] A Caddyfile whose marker version is below 9 fails when the Tamanu version is 2.46.0 or greater.
- [ ] A Caddyfile whose marker version is below 9 warns when the Tamanu version is below 2.46.0.
- [ ] A Caddyfile with no valid first-line marker fails, regardless of the Tamanu version.

The version threshold (9) and the Tamanu version boundary (2.46.0) are fixed in the check.

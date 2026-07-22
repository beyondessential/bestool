# Caddyfile version healthcheck — test cases

Covers the `caddyfile_version` doctor check (verifies spec: CHK-CFV).

## Marker parsing

- [x] `# tamanu caddyfile v9` parses as version 9
- [x] A CRLF line ending (`# tamanu caddyfile v9\r`) still parses
- [x] Multi-digit versions parse (`# tamanu caddyfile v12` → 12)
- [x] Trailing content after the number is rejected (`# tamanu caddyfile v9 beta`)
- [x] Wrong prefix, wrong case, other comments, and an empty line are rejected

## Outcomes (verifies spec: CHK-CFV)

- [x] Marker version ≥ 9 passes on any Tamanu version
- [x] Missing/invalid marker fails regardless of Tamanu version
- [x] Marker version < 9 fails when Tamanu ≥ 2.46.0 (including the 2.46.0 boundary)
- [x] Marker version < 9 warns when Tamanu < 2.46.0

## Applicability (manual / integration)

- [ ] Skips on a non-Windows host
- [ ] Skips on a Windows host with no Tamanu deployment
- [ ] Skips on a Windows Tamanu host with no Caddyfile on disk (caddy not present)
- [ ] Reads `C:\Caddy\Caddyfile` / `Caddyfile.txt` on a real Windows install

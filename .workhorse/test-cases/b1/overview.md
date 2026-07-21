# Canopy registration healthcheck

Scenarios for the `canopy_registration` doctor check. Grading cases are covered by the check module's unit tests over the pure `grade` function; the end-to-end and reporting cases are for manual verification on a real host.

## Grading

- [x] No registration record on the host fails, directing the operator to run `bestool canopy register` (verifies spec: REG).
- [x] A registration missing the server id fails (verifies spec: REG).
- [x] A registration missing the device id fails, citing rejected backups (verifies spec: REG).
- [x] A registration missing only the device key warns, citing the tailscale fallback (verifies spec: REG).
- [x] A registration missing only the API URL still passes (verifies spec: REG).
- [x] A fully-populated registration passes (verifies spec: REG).
- [x] When both a fatal field (device id) and a soft field (device key) are absent, the outcome is the most severe (fail) and both reasons are carried (verifies spec: REG).

## End-to-end and reporting

- [ ] `bestool tamanu doctor` runs the check on a host with no Tamanu install and shows its outcome.
- [ ] A registration that exists but cannot be decrypted (e.g. a cloned disk) reports broken rather than fail.
- [ ] The check's outcome and field-presence details appear in the payload reported to Canopy.

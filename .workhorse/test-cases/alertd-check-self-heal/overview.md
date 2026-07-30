# Self-healing healthchecks + canopy registration identity recovery

Scenarios verifying the generic heal framework ([CHK](../../specs/tamanu/healthchecks.md#self-healing)) and the canopy_registration identity recovery ([REG](../../specs/canopy/registration.md#recovering-a-missing-identity)).

## Heal framework

- [x] Backoff grows from the minimum interval, doubling, and caps at the maximum rather than growing without bound (CHK)
- [x] At most one heal attempt for a check runs at a time: a second is refused while the first is in flight (CHK)
- [x] A deferred attempt backs off, so the next sweep does not retry immediately (CHK)
- [ ] The one-shot `doctor` command never attempts a heal, so running it by hand has no side effects (CHK)
- [ ] A non-passing check without a heal action is left untouched (CHK)
- [ ] A passing check is never healed (CHK)

## Canopy registration recovery

- [x] Recovery fills only a missing device id and leaves a present server id, the device key, and the API URL untouched (REG)
- [x] Recovery fills a missing server id and leaves a present device id (REG)
- [x] A store at the default location refreshes the process cache, so a recovered identity is read by the next in-process load without a restart (REG)
- [ ] With a reachable Canopy, a registration missing a device id is completed from `GET /servers/self` and a later sweep passes (REG) — end-to-end, gated on the canopy endpoint being deployed
- [ ] When Canopy is unreachable or returns no identity (unknown device, not attached, attached to several), no identifiers are written and the reported outcome is unchanged (REG)

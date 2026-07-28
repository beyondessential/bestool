---
id: REG
---

# Canopy registration health

A host's Canopy enrolment is held in a single registration record: the server id it backs up as, the device mTLS key, the device id assigned at enrolment, and the Canopy API URL.
`bestool canopy register` populates all four; a host carried over from older per-file state has only the server id and device key.

A doctor healthcheck grades this registration so an operator sees an incomplete or older-format enrolment before it matters downstream — most relevantly on a deployment that has Canopy backups configured, where a snapshot is tagged with the device id (see [BAK](backup.md)).

## The registration healthcheck

The `canopy_registration` check is one of the doctor's healthchecks; see [DOC](../tamanu/doctor.md) for the framework it runs in.
The check runs on every host, whether or not Tamanu is installed and whether or not backups are enabled, so an incomplete enrolment surfaces ahead of the work that depends on it.
It reaches its verdict from the local registration record alone, and reports its outcome to Canopy alongside the other healthchecks.
When several of the conditions below hold at once, the check reports the most severe outcome.

## Outcomes

With no registration record on the host, the check fails: the host is not enrolled, and the reason directs the operator to run `bestool canopy register`.
With a registration that has no server id, the check fails: the host is on an older Canopy registration format, which updating bestool usually migrates to the current format.
With a registration that has no device id, the check fails for the same reason; updating bestool usually migrates it, and a manual `bestool canopy register` is needed only if that does not resolve it.
This affects backups only on a deployment that has Canopy backups configured.
With a registration that has no device key, the check warns, because the host authenticates to Canopy over the tailscale path rather than by mTLS.
With a server id, a device id, and a device key all present, the check passes; the API URL is not required, because a registration without one falls back to the default Canopy URL.

## Recovering a missing identity

A registration can be missing the server id or the device id assigned at enrolment — for instance a host carried over from older per-file state, which holds only the server id and device key.
When the check fails for a missing server id or device id, it attempts to recover the missing identifier from Canopy as a self-heal action (see [CHK](../tamanu/healthchecks.md#self-healing)).

Canopy resolves a device's identity from the authentication the device already presents — its tailnet identity or its mTLS device certificate — so a host that has lost track of its own identifiers can ask Canopy for them without knowing them first.
The check reads them from Canopy's device self-identity endpoint, `GET /servers/self`, which returns the server id the host is enrolled as together with its own device id.
The recovered server id and device id are written into the registration record; the device key and API URL are left untouched.

Recovery is attempted only when Canopy is reachable by one of those authentication paths.
When Canopy cannot be reached, or reports that the presented identity matches no known device, is registered but not yet attached to a server, or is attached to more than one, no identifiers are written and the check's reported outcome is unchanged.
Once the identifiers are recovered, a later sweep reads the completed registration and the check passes without operator action or a daemon restart.

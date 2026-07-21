---
id: REG
---

# Canopy registration health

A host's Canopy enrolment is held in a single registration record: the server id it backs up as, the device mTLS key, the device id assigned at enrolment, and the Canopy API URL.
`bestool canopy register` populates all four; a host carried over from older per-file state has only the server id and device key.

A doctor healthcheck grades this registration so an operator sees an incomplete enrolment before it causes a failure downstream — most importantly before backups are enabled, since Canopy rejects a backup snapshot that is not tagged with a device id (see [BAK](backup.md)).

## The registration healthcheck

The `canopy_registration` check is one of the doctor's healthchecks; see [DOC](../tamanu/doctor.md) for the framework it runs in.
The check runs on every host, whether or not Tamanu is installed and whether or not backups are enabled, so an incomplete enrolment surfaces ahead of the work that depends on it.
It reaches its verdict from the local registration record alone, and reports its outcome to Canopy alongside the other healthchecks.
When several of the conditions below hold at once, the check reports the most severe outcome.

## Outcomes

With no registration record on the host, the check fails: the host is not enrolled, and the reason directs the operator to run `bestool canopy register`.
With a registration that has no server id, the check fails, because the host cannot identify itself to Canopy.
With a registration that has no device id, the check fails, because backups are rejected until the host is re-enrolled with `bestool canopy register`.
With a registration that has no device key, the check warns, because the host has no mTLS identity and depends on the tailscale path to carry its authentication.
With a server id, a device id, and a device key all present, the check passes; the API URL is not required, because a registration without one falls back to the default Canopy URL.

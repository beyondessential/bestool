# Name the application on canopy DNS name requests, and wait on an undeclared refusal

## Dependency on canopy F4

- Needs the bes-canopy-api release carrying F4: optional `application_type` on `RequestCertificateArgs` and `RegisterNameArgs`, and `CanopyHttpError`'s message including the problem detail. Bump with `cargo add`, don't guess the version.
- The problem type is in the body `CanopyHttpError` already carries (`body`), as a problem document whose `type` is `/errors/dns-name-undeclared` or `/errors/dns-name-denied`. Branch on that, not on the 403 alone: `name-not-entitled` is also 403.
- F4 hasn't settled the problem type for a type that mismatches an existing declaration. Bestool doesn't need it: anything other than undeclared/denied is graded as a failure, with canopy's detail as the reason. NAM and CHK-CCO still assert the refusal names the declaring type; confirm against F4 before implementing.
- On canopy's side `registered_names` is built from every declaration row (`ApplicationName::for_server`), so it already means "DNS names this application declares", including operator declarations with no addresses. Ownership's declaration fallback and the check's daemon-unreachable fallback both read it as that.

## Ownership (NAM#which-application-a-dns-name-belongs-to)

One function, shared by the daemon and both checks: DNS name → owning application, or none.

1. The application its Caddy site is attributed to.
2. Otherwise the application whose `registered_names` lists it.
3. Otherwise none, and the DNS name is ignored: not requested, not graded by either check.

Daemon effects:

- `target_names` keeps only owned DNS names, and tests `may_certify` against the owner's `AppEntitlement`, not the union. The union stays for `stands_down` (`holds_tls_grant`, `fully_paused`).
- Every daemon request carries the owner's type. There is no "no type" request from a pass any more.
- Handshake-recorded names go through the same ownership test as configured ones.

### Site attribution

Read from Caddy's live admin JSON, per site (server route matching a host):

- Canonical URL first: a site whose host matcher includes a Tamanu's `canonical_host_name` / `canonical_url` host (`crates/tamanu/src/config/structure.rs`) is that Tamanu's.
- Otherwise upstreams. Linux (Ansible layout): `dynamic_upstreams` A lookups through Podman DNS. Tamanu: `api.<central|facility>.tamanu.internal`, `frontend.tamanu.internal`, `patientportal.tamanu.internal`, port 3000. mSupply: `api.msupply.internal`, port 8000. Site files are `/etc/caddy/servers/{tamanu,tamanu-patientportal,msupply}`. Windows: `reverse_proxy localhost:<TamanuPort>`; no mSupply on Windows.
- `frontend.tamanu.internal` doesn't carry the role. With more than one Tamanu on the host, attribute by the `/api/*` route's upstream instead.
- No upstream (redirects, `file_server`), mixed applications, or an unrecognised upstream → unattributed unless the canonical URL matched. Watch the vacuous "every upstream" case.
- A DNS name served by sites attributed to different applications → unattributed.

## Explicit commands carry the application

- `bestool canopy certs request` and `bestool canopy dns register` take a required application type argument, passed to the daemon's `request` / `dns-register` endpoints and on to canopy as `application_type`.
- `dns withdraw` stays name-only and sends no type.
- The daemon's `request` endpoint checks the domain against the named application's domains, not the union.

## Per-site Caddy hook (TLS#which-dns-names-are-certified)

- `delivery::caddy_subjects()` and the sweep cache's `caddy_subjects` return every site address. Both need to keep only addresses whose TLS automation policy has a `get_certificate` manager of `via: http` pointing at the daemon's `/certificate`.
- Applies to the daemon's `target_names` and to the check's graded set.
- Exception for explicit requests: ordered regardless of hook or whether Caddy serves the name. Today `pass()` drops a name from `wanted` on any `Ok`, including pending, so a pending order for a name outside Caddy's hooked subjects is never collected. Keep an explicitly requested name targeted until its first chain arrives, then while Caddy serves it (hooked or not).

## Daemon refusal record (TLS#undeclared-and-denied-dns-names)

- `Order` gains the refusal kind (undeclared / denied / other), canopy's detail, and the application type the request carried. Today a failed request leaves `state` empty, which is what produced `chuuk... — : asking canopy...` in F4's report.
- Persist the refusal kind and detail with the rest of the on-disk state so it survives a restart.
- `prune_orders` already drops orders for names no longer targeted; the refusal goes with them.
- `name_due` must not treat a name with an undeclared or denied refusal as due between steady passes, even if it's in `wanted`. Same shape as the stand-down guard, but per name.
- `report()` carries the refusal kind, detail and type sent, for the open `status` endpoint, which the check and `bestool canopy certs` both read.
- A denied name keeps any held chain until expiry. `serve()` turns on the chain in hand already, so the only thing to watch for is that no new code path drops it.

## Check (CHK-CCO)

- Graded set: certified DNS names owned by this application.
- Asks the daemon's `status` endpoint once per sweep (sweep cache), like the entitlement. If the daemon is unreachable, it grades only this application's declared names (`registered_names`) and says so in the detail.
- Undeclared and denied are both detail-only: listed, not graded, no effect on outcome or summary. No warning.

## mSupply application (SUBJ)

- New `ApplicationKind::Msupply`, type slug `msupply`, detected by `/etc/containers/systemd/msupply.container`.
- `canopy_certificates` and `caddy_certs` apply, both currently wired against `tamanu_app`; each needs a selector covering Tamanu and mSupply. Facts are type and version: the version comes from `MSUPPLY_VERSION` in `/etc/msupply/env` (e.g. `v2.17.06-sqlite-amd64` → `2.17.06`).
- `msupply` is the first use of an mSupply type anywhere, so this card sets it. Canopy's mSupply applications need to be recorded under the same slug, since CHK-CCO matches entries by type and requests carry it. Raise it on the canopy side so F4 (or its follow-up) doesn't pick another spelling.

## caddy_certs attribution (CHK-CCT)

- Grades per application using the shared ownership function. A certificate is graded under every application owning one of its DNS names. One whose DNS names are all unowned is not graded.
- The comment at `crates/alertd/src/checks.rs` near the `canopy_certificates` entry, about attributing "more finely than `caddy_certs` manages", goes.

## Code references to update

Headings renamed:

- `NAM#registering-addresses-for-a-name` → `NAM#registering-addresses-for-a-dns-name` (`crates/bestool/src/alertd/certificates.rs`)
- `TLS#which-names-are-certified` → `TLS#which-dns-names-are-certified` (`crates/bestool/src/alertd/certificates.rs`, `crates/bestool/src/alertd/certificates/delivery.rs`)
- `CHK-CCO#which-names-it-grades` → `CHK-CCO#which-dns-names-it-grades` (`crates/alertd/src/sweep_cache.rs`, `crates/alertd/src/checks/canopy_certificates.rs`, `crates/canopy/src/names.rs`)

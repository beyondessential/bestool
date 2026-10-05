# Name the application on canopy DNS name requests, and wait on an undeclared refusal

## Dependency on canopy F4

- Needs the bes-canopy-api release carrying F4: optional `application_type` on `RequestCertificateArgs` and `RegisterNameArgs`, and `CanopyHttpError`'s message including the problem detail. Bump with `cargo add`, don't guess the version.
- The problem type is in the body `CanopyHttpError` already carries (`body`), as a problem document whose `type` is `/errors/dns-name-undeclared` or `/errors/dns-name-denied`. Branch on that, not on the 403 alone: `name-not-entitled` is also 403.
- F4 hasn't settled the problem type for a type that mismatches an existing declaration. Bestool doesn't need it: anything other than undeclared/denied is graded as a failure, with canopy's detail as the reason.
- On canopy's side `registered_names` is built from every declaration row (`ApplicationName::for_server`), so it already means "DNS names this application declares", including operator declarations with no addresses. The check's daemon-unreachable fallback reads it as that.

## Site attribution (NAM#which-application-a-site-belongs-to)

Read from Caddy's live admin JSON, per site (server route matching a host):

- Linux (Ansible layout): upstreams are `dynamic_upstreams` A lookups through Podman DNS. Tamanu: `api.<central|facility>.tamanu.internal`, `frontend.tamanu.internal`, `patientportal.tamanu.internal`, port 3000. mSupply: `api.msupply.internal`, port 8000. Site files are `/etc/caddy/servers/{tamanu,tamanu-patientportal,msupply}`.
- Windows: `reverse_proxy localhost:<TamanuPort>`. No mSupply on Windows.
- Tamanu's canonical host comes from `canonical_host_name` / `canonical_url` in its config (`crates/tamanu/src/config/structure.rs`).
- `frontend.tamanu.internal` doesn't carry the role. On a host running both central and facility, a site whose only upstream is the shared frontend is attributed by its `/api/*` route's upstream, or by the canonical host. If that's still ambiguous, send no type.
- The same attribution feeds `dns-register` / `dns-withdraw`, which currently get only a name from the CLI.

## Per-site Caddy hook (TLS#which-names-are-certified)

- `delivery::caddy_subjects()` and the sweep cache's `caddy_subjects` return every site address. Both need to keep only addresses whose TLS automation policy has a `get_certificate` manager of `via: http` pointing at the daemon's `/certificate`.
- Applies to the daemon's `target_names` and to the check's graded set.
- Exception: a name the `request` endpoint was asked for is ordered regardless of the hook. Passes keep collecting it while Caddy serves it and a chain is held, so `target_names` has to keep held names in Caddy's subjects even when their site isn't hooked.

## Daemon refusal record (TLS#undeclared-and-denied-dns-names)

- `Order` gains the refusal kind (undeclared / denied / other), canopy's detail, and the application type the request carried. Today a failed request leaves `state` empty, which is what produced `chuuk... — : asking canopy...` in F4's report.
- `name_due` must not treat a name with an undeclared or denied refusal as due between steady passes, even if it's in `wanted`. Same shape as the stand-down guard, but per name.
- `report()` carries the refusal kind, detail and type sent, for the open `status` endpoint, which the check and `bestool canopy certs` both read.
- A denied name keeps any held chain until expiry. `serve()` turns on the chain in hand already, so the only thing to watch for is that no new code path drops it.

## Check (CHK-CCO)

- Asks the daemon's `status` endpoint once per sweep (sweep cache), like the entitlement. If the daemon is unreachable, it grades only this application's declared names (`registered_names`) and says so in the detail.
- Undeclared → warn (unless something else fails); denied → detail only.

## mSupply application (SUBJ)

- New `ApplicationKind::Msupply`, type slug `msupply`, detected by `/etc/containers/systemd/msupply.container`.
- `canopy_certificates` and `caddy_certs` apply, both currently wired against `tamanu_app`; each needs a selector covering Tamanu and mSupply. Facts are type and version: the version comes from `MSUPPLY_VERSION` in `/etc/msupply/env` (e.g. `v2.17.06-sqlite-amd64` → `2.17.06`).

## Code references to update

Headings renamed for the "DNS name" wording:

- `NAM#registering-addresses-for-a-name` → `NAM#registering-addresses-for-a-dns-name` (`crates/bestool/src/alertd/certificates.rs`)
- `CHK-CCO#which-names-it-grades` → `CHK-CCO#which-dns-names-it-grades` (`crates/alertd/src/sweep_cache.rs`, `crates/alertd/src/checks/canopy_certificates.rs`, `crates/canopy/src/names.rs`)

## caddy_certs attribution (CHK-CCT)

- Grades per application using the same site attribution as NAM. A certificate is graded under every application whose site it serves. One serving only unattributed sites falls back to the host's Tamanu.
- The comment at `crates/alertd/src/checks.rs` near the `canopy_certificates` entry, about attributing "more finely than `caddy_certs` manages", goes.

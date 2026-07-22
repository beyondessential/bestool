---
id: CHK
---

# Healthchecks

The doctor and the alertd daemon run a shared registry of named healthchecks against a host and its Tamanu deployment. Each check resolves to one of the outcomes and is selected, ordered, and rendered as described in `tamanu/doctor.md`.

This spec is the parent for the healthcheck catalogue: the conventions common to every check, with each check that warrants its own acceptance criteria captured in a sibling spec.

## Spec identifiers

Every spec describing an individual healthcheck carries a frontmatter `id` of the form `CHK-<id>`, where `<id>` is a short identifier for that check (for example `CHK-CFV` for the Caddyfile version check). The shared `CHK-` prefix distinguishes healthcheck specs from other specs at a glance and groups them for code-to-spec traceability.

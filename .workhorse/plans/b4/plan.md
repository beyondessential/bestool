# Local identity token endpoint

alertd serves `GET /identity-token?audience=…&application_type=…` so workloads on a box can obtain Canopy identity tokens without holding a long-lived credential.
Canopy's side is IDT (`.workhorse/specs/public-server/identity-tokens.md` in the Canopy repository), served as `POST /identity-tokens`.
This plan covers bestool's side only.

## Canopy client

`bes-canopy-api` 1.1.0 is published and 1.1.2 is pending (canopy#667, a version bump over the already-merged endpoint).
The client has `identity_tokens(&IdentityTokenArgs { audience, application_type })` returning `IdentityTokenResponse { token, expires_at, subject }`.
The crate has to be bumped past the release that carries the endpoint before implementation starts.

A refusal arrives as `Error::Http(CanopyHttpError { status, body, .. })`, so a refusal passes through as the status and the body bytes Canopy sent, with its content type, and the workload sees the problem document unchanged.
Failures that never reached Canopy (no registration, no device key, Canopy unreachable) are the daemon's own and need their own problem document in the same shape, with problem types of the daemon's own.

Each request builds on the daemon's existing Canopy connection and makes exactly one call.
Nothing is cached and nothing is retried, because Canopy mints a new `jti` per call and relying parties accept each once.
A success returns Canopy's JSON unchanged.

## Route and listeners

The route is `GET /identity-token`, with `audience` required and `application_type` optional.
A missing `audience` is refused by the daemon before any call to Canopy.

Two listeners serve it, because the two kinds of caller reach the daemon over different networks:

- the existing loopback listener on 8271, for callers on the host;
- a new listener on the podman bridge gateway, for containers.

The bridge listener serves a router holding `/identity-token` and nothing else.
The main router's `/status`, `/metrics`, `/reload` and the task endpoints carry no caller check beyond the guarded ones, so mounting it on an address containers can reach would widen what any container can see and do.

## Who may ask

### Host callers

Host callers are identified the way `/certificate` identifies Caddy: the owner of the connection's socket from `/proc/net/tcp` and `/proc/net/tcp6`, admitting root plus users named in the daemon's configuration.
The workloads' service users are configured separately from Caddy's, so this is a new option beside `--permit-cert-user`, and it takes more than one user.
`peer::Permitted` carries one extra uid today and needs to carry a set, with the certificate path continuing to pass one.

Only host callers may ask for a token naming the machine, which is the request with no `application_type`.

A container started with host networking connects over loopback, so the host's socket table lists it under the uid it runs as inside the container, which is root unless it drops privilege.
Such a container is a host caller and is admitted as one.

### Container callers

A container is identified by its source address, resolved to the container that holds it through the rootful podman socket the daemon already talks to.
A container's client socket lives in its own network namespace, so the host's socket table does not list it and the uid check cannot name it.

A container takes part by carrying a label that names the one application type it may obtain tokens for, set by whatever runs the container (the deployment's quadlet).
A container without the label is refused.
No attribution from unit names is needed, and the role-neutral units (frontend, patient portal) are simply not opted in unless the deployment labels them.

A container may request only the application type its label names, and a request naming any other is refused by the daemon without asking Canopy.
A request from a container that omits `application_type` is refused, so a container cannot act as the machine.

Resolution happens on every request.
A container restarted by systemd takes a new address, so a remembered address could name the wrong container.
The lookup matches the connection's source address against the addresses of labelled containers on the network whose gateway the connection arrived at, which keeps two networks with overlapping ranges apart.
Labelled containers are few and requests are infrequent, so the cost is one list and a handful of inspects.

The trust this rests on: a container cannot take another container's address on the bridge.
A container holding the capability to forge frames could, and so could anything able to create a container on the rootful socket, which is already root.

## Platforms

Windows workloads run as services, with no container and no bridge.
Identifying their caller needs the owner lookup through the TCP table and a model of service users, which is the same work the certificate endpoint is waiting on.
Until that lands the endpoint refuses callers on Windows rather than serving anyone, as `/certificate` does.

## Open

- The label's key, and whether its value is the bare application type slug.
- How the bridge listener learns its addresses. The default podman network's gateway is predictable, but the bridge may not exist when alertd starts, and networks can be added later. Candidates are binding with `IP_FREEBIND` to gateways read from podman at start and on reload, or rescanning on an interval.
- The daemon's own problem types and where they are specified. Canopy's are listed in the card; the daemon adds at least: caller not permitted, container not opted in, application type not the container's, no registration to authenticate with.
- Where this is specified in bestool. A new spec under `.workhorse/specs/canopy/` is likely, with the certificate spec's "Who may fetch a certificate" linking to it for the shared caller identification.
- Whether the audience should be restricted per caller, or left to Canopy's grants.

---
id: NODE
---

# Node.js version reporting

The server facts gathered by the doctor and pushed by the alertd daemon carry the Node.js runtime version the Tamanu server runs under, reported as `nodeVersion`.
This is the version of the runtime Tamanu actually executes under, not merely a runtime that happens to be installed on the host.

The doctor renders these facts and includes them in its machine-readable payload (`tamanu/doctor.md`), and the alertd status push carries the same value to Canopy.

## Source

On a containerised deployment, the reported version is the Node.js version inside a running Tamanu server container.
The host that runs the daemon need not have Node.js installed, and the container carries its own runtime, so the container is the only place the true running version can be observed.

On a deployment driven by a process manager with a bundled Node.js runtime, the reported version is that bundled runtime's version.
The runtime ships in a `runtime` directory at the deployment's server root, and the process manager launches the server with it, so its version is the version Tamanu runs under.

When neither a running container nor a bundled runtime is found — for example on a developer machine or a partially provisioned host — the reported version falls back to the Node.js version found on the host's search path.

When no Node.js runtime can be resolved by any of these means, `nodeVersion` is omitted from the reported facts.

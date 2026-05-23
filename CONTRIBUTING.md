# bestool Contributions Guide

Thank you for taking the time to contribute, and welcome to this open-source project!

## Code of Conduct

This project and everyone participating in it is governed by the
[BES Open Source Code of Conduct](CODE_OF_CONDUCT.md).
By participating, you are expected to uphold this code. Please report unacceptable behavior
to [opensource@tamanu.io](opensource@tamanu.io).

## Commit Convention

The subject (first line) of commit messages must be in [Conventional Commit](https://www.conventionalcommits.org/en/v1.0.0/)
format. This is used for version bumps on releases and also for general historical purposes.

```plain
type: <description>
type(scope): <description>
```

When a Linear card is applicable, the Linear card number should be included:

```plain
type: TEAM-123: <description>
type(scope): TEAM-123: <description>
```

## Releasing

Releases are automated by [release-plz](https://release-plz.dev). Pushing to `main` opens a `repo: release` PR with per-crate version bumps determined from conventional commits and `cargo-semver-checks`; merging that PR (auto-merge is enabled once CI is green) publishes the affected crates to crates.io, pushes per-crate tags, and triggers the binary-build workflows.

### Holding a release

To bundle several features or fixes into one release, add the `release-hold` label onto the open `repo: release` PR. That will hold the PR open until you either:
- merge it manually, or
- remove the label and merge another PR.

## License


Any contributions you make will be licensed under [the General Public License version 3.0](./COPYING).


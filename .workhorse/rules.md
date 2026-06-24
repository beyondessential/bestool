# Spec rules

Specs live in `.workhorse/specs/<area>/<name>.md`.
Each carries an `id` in its frontmatter (e.g. `BAK`, `S3P`) and is cross-referenced by linking the file under that id, e.g. `[BAK](backup.md)`.

A spec is the durable description of **what** the system requires.
It is read by someone deciding whether the implementation is correct, or re-implementing the feature from scratch — not as a narrative of how the code works or how it came to be.

Specs are written in markdown prose with each sentence on its own line and no hard-wrapping.
This balances ease of writing and diff parseability.

## What, not how

- Describe **what** the system requires, not **how** the code achieves it.
  Keep out of spec text: tool and command names (`sfdisk`, `kopia snapshot create`), crate and library names, syscall names (`splice(2)`), internal API details, data-structure choices, and environment variable names used only by the implementation.
- Acceptable, because external actors or other components depend on them: interface contracts — config file paths and formats, on-disk and on-the-wire shapes, endpoint shapes, partition UUIDs, credential scopes.
- The test: would someone re-implementing the feature from scratch be constrained to the same choice?
  If not, it's an implementation detail and doesn't belong in the spec.

## Present, not past

- State what the system does, not how it got there.
  No "this supersedes X", "formerly Y", "a spike settled Z", changelog entries, or migration narration.
- When something is removed or replaced, edit the spec to describe the new reality and delete the old text, rather than describing the transition.
  The git history is the record of change; the spec is the record of the present.

## Own behaviour, not a dependency's internals

- Describe the system's own behaviour and contracts: the request shape it handles, the guarantee it makes.
  Don't narrate a dependency's decision logic or version-specific quirks beyond the minimum needed to justify a requirement.
- Don't scaffold or label: no "Strategy A/B", "Phase N", or plan tags in spec prose.
  Describe the mechanism directly.

## Workflow

- When adding or changing a feature, or fixing a bug: update the spec in `.workhorse/specs` first, then implement, then add tests.

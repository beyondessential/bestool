# Spec and plan rules

Specs live in `.workhorse/specs/<area>/<name>.md`; plans live in `.workhorse/plans/`.

For the spec file format, the frontmatter `id`, code-to-spec traceability, the fold/create/split decision, and when a change warrants a spec update, follow `.agents/docs/spec-format.md` and the Workhorse skills.
This file records only the bestool house conventions that sit on top of that.

A spec is the durable description of **what** the system requires.
It is read by someone deciding whether the implementation is correct, or re-implementing the feature from scratch — not as a narrative of how the code works or how it came to be.

## House style

Specs are written in markdown prose with each sentence on its own line and no hard-wrapping, rather than the checkbox acceptance-criteria style shown in `spec-format.md`.
This balances ease of writing and diff parseability.

## Cross-references

Specs reference each other with markdown links under the target's id, e.g. `[BAK](backup.md)`.
Code references the spec it implements with an inline `// spec: BAK` comment, as described in `spec-format.md`.

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

## Plans and the unplan lifecycle

A plan captures the design and outstanding work for a feature while it is being built.
Plans are point-in-time working documents: open questions, options, and trade-offs are welcome, unlike in specs.

- A plan is added in a `plan:` commit and lives in `.workhorse/plans/` until its work is implemented.
- Once the work has shipped, the plan is deleted in an `unplan:` commit.
  If the feature has durable behaviour worth recording, fold it into a spec under `.workhorse/specs/` in that same commit, then delete the plan.
- A plan that documents a genuinely undecided question may stay until the decision is made.

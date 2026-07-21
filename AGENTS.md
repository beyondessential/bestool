<!-- BEGIN:workhorse 0.1.0 -->
# Workhorse framework

This workspace uses [Workhorse](https://github.com/beyondessential/workhorse), a spec-driven development workbench. Workhorse ships skills (invokable prompts) and reference docs into this repo to shape how AI agents work here.

- **Skills** live at `.agents/skills/` — each skill is a folder containing a `SKILL.md` with YAML frontmatter and a prompt body. `.claude/skills/` is a symlink to the same folder so Claude Code picks them up natively
- **Reference docs** live at `.agents/docs/` — long-form guidance that skill bodies cite by path (spec format conventions and similar)
- **Specs** live at `.workhorse/specs/` — acceptance criteria for each piece of work, organised into areas by subdirectory

When picking up a task, read the skill whose folder name matches what you're being asked to do — its `SKILL.md` describes how to approach the work and which reference docs to follow.

Workhorse manages this section. Run the **Pull Workhorse updates** skill to bring it, the skills, and the reference docs up to the latest release — local edits you make here are preserved through a smart merge. Edit or remove it freely.
<!-- END:workhorse -->

## LLM Rules to follow
- Use jj/jujutsu locally when available and enabled for the repo.
- NEVER hardcode database credentials. Always use the environment variable or existing client.
- When adding or changing features, or when fixing bugs, add tests whenever possible.
- Never write documentation files or readmes.
- Work from a branch. If you're on main, branch off.
- Always run `cargo clippy` and `cargo fmt` before committing changes.
- Use conventional commit messages. Add a Co-authored-by: line or similar.
- Never write useless comments that only repeat the code.
- Never print summaries or unnecessary information.
- Don't use emojis unless absolutely necessary.
- When removing code that has already been committed, delete it unless explicitly requested that it be commented out.
- Prefer using small dependencies instead of reimplementing the wheel. Ask the user to pick a dependency if there is no obvious choice.
- Imports: merge them and group them by std, then third-party/workspace, then local (crate, super, self).
- Ask the user instead of making an assumption if there's a major detail missing from instructions that could affect code quality or implementation design.
- When writing parsers, unless very trivial, implement them using winnow or chumsky.
- Use the newer `foo.rs` / `foo/sub.rs` style of modules.
- `use` statements always go before `mod` statements.
- ALWAYS use the edit tool to edit or write file, NEVER use "cat >> EOF". YOU WILL LOSE DATA.
- Never write long summaries at the end of responses. Maximum 50 words if absolutely necessary.
- To silence a warning, use `#[expect(..., reason = "...")]` instead of `#[allow(...)]`.
- When changing Windows-specific code, run `cargo check` with a Windows GNU target (unless currently running on Windows).
- Releases are automated via release-plz: pushing to `main` opens a `repo: release` PR which auto-merges and publishes to crates.io. No manual release step is needed.
- It's very important for alertd that postgres (or anything else we're checking) IS NOT REQUIRED for the alertd daemon to start, because otherwise we CANNOT ALERT ON THE DATABASE BEING DOWN.
- The alertd daemon is run using the exact unit file in services/bestool-alertd.service.
- When writing or changing specs in `.workhorse/specs/` or plans in `.workhorse/plans/`, follow the spec and plan rules in [.workhorse/rules.md](.workhorse/rules.md).
<!-- end rules -->

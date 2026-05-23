# `tamanu logs` multi-name + caddy unification

Follow-up to the lifecycle PR (`tamanu-lifecycle.md`). Reshapes
`tamanu logs` to share the matcher surface introduced there, and folds
caddy log tailing into the same code path as tamanu service logs.

Linear: TAM-6782 (same ticket as lifecycle).

## Why a separate PR

The lifecycle PR introduces `lifecycle::match_names` and the
`Criticality` field. It's already a large change. Reshaping `logs.rs`
on top of those primitives is mechanically straightforward but is its
own concern (operator UX rather than service lifecycle), and the diff
touches a different file (`logs.rs`) than the lifecycle work. Splitting
keeps each PR reviewable on its own.

## Changes

- `LogsArgs.name: String` → `LogsArgs.names: Vec<String>`.
- Empty `names` = every expected-Up tamanu service plus caddy.
- Each name in `names` is a substring match via
  `lifecycle::match_names`. The literal pseudo-service `caddy` is
  recognised as a named match in addition to tamanu expectations.
- `--grep`, `-n`, `-f` flags unchanged in shape.

## Behaviour

On Linux the union collapses to a single `journalctl` call:

```
journalctl -u <unit1> -u <unit2> ... [-u caddy.service] -n N [-f] [-g REGEX] --output=cat
```

The existing JSON highlighter in `format_log_line` is content-based
(already opportunistic per line), so caddy lines get highlighted and
tamanu lines don't, in the same stream.

On Windows the existing `tail_files` already takes multiple
`TailSource`s. Extend the source collection so when caddy is in the
matched set, `caddy_log_files_windows()` contributes its `.log` files
alongside the pm2 sources from `pm2::log_sources()`.

## Error UX

- Any name that matches zero expectations and is not the literal
  `caddy`: bail with the available names (same UX as the lifecycle
  commands).
- The literal `caddy` always matches; it's not subject to discovery.

## Testing

- Unit tests on the args parse: zero names, one name, multiple names,
  `caddy` alongside tamanu names.
- Integration testing is manual (real journalctl / pm2 logs streams).

## PR

Title: `feat(tamanu/logs): TAM-6782: multi-name + unified caddy tailing`

## Stacking

Stack on top of the lifecycle PR. Once that merges, this one
auto-rebases to main.

# psql plain & sql output formats

Add two output formats to the psql REPL, modelled on the existing csv/json/expanded
machinery.

## Goals

1. **plain**: prints the first (selected) column of every row as a bare string, one per
   line, no formatting/borders/header. Warns to stderr if more than one column is
   selected (then prints only the first). Reachable via `\re show format=plain` and the
   `\gp` shorthand.
2. **sql**: formats the result as SQL `INSERT` statements. Reachable via
   `\re show format=sql` and the `\gs` shorthand.
   - default sql: a single `INSERT INTO <table> (cols) VALUES (..), (..), ..;`
   - **expanded sql**: one `INSERT` statement per row. Reachable via
     `\re show format=sql-expanded` and `\gsx` (sql + expanded, mirroring `\gj`/`\gjx`).

## Table name

Derived from the query's FROM clause via `pg_query` (the first `RangeVar` in the first
`SelectStmt`'s from_clause, schema-qualified if a schema is present). When nothing
satisfactory can be derived (subquery/VALUES/no FROM/join with non-RangeVar leftmost),
fall back to the fixed default `results`. Emit a stderr warning when falling back.

## Value formatting (SQL literals)

New helpers in `crates/postgres/src/stringify.rs`:
- `is_null(row, idx) -> bool`: type-aware NULL detection, matching on the column
  `Type` (mirrors `postgres_to_json_value`'s type match) using `Option<T>`.
- `sql_quote(ty: &Type, text: &str, is_null: bool) -> String`:
  - null → `NULL`
  - numeric types (INT2/4/8, FLOAT4/8, NUMERIC, OID) → unquoted text
  - BOOL → `TRUE`/`FALSE`
  - everything else → `'…'` with single quotes doubled

The sql renderer resolves each cell's text exactly like csv.rs (redaction, unprintable
text-cast via `TextCaster`, else `get_value`), checks `is_null` first, then `sql_quote`.

Identifiers (column names, table name parts) emitted as-is when they match
`^[a-z_][a-z0-9_]*$`, otherwise double-quoted with `"` doubled.

## Changes

### Formats / enums
- `parser/metacommands/result.rs`:
  - add `ResultFormat::Plain`, `Sql`, `SqlExpanded`.
  - parse `format=plain|sql|sql-expanded`.
  - `is_file_only` unchanged (these are screen-capable).
- `parser/query_modifiers.rs`:
  - add `QueryModifier::Plain`, `QueryModifier::Sql`.
  - `modifier_char`: add `p` → Plain. Add `s` → Sql, guarded with
    `(literal('s'), not(literal("et")))` so `\gset` still parses as the set keyword.
  - map chars to modifiers in the apply loop.

### Renderers
- new `query/display/plain.rs`: `display(ctx)` — first selected column only, warn if >1.
- new `query/display/sql.rs`: `display(ctx, expanded: bool, table: &str)`.
- `query/display.rs`: declare `mod plain; mod sql;`, add `display_plain` and
  `display_sql` wrappers (mirroring `display_csv`).

### Table derivation
- `column_extractor.rs`: add `derive_table_name(sql: &str) -> Option<String>` using
  the existing `pg_query` parse.

### Dispatch sites
- `query.rs` (immediate `\g` path): branch on Plain / Sql(+Expanded) before the existing
  json/expanded dispatch; derive table from `statement`; keep the existing 50→30
  auto-truncation. Format modifier precedence when combined: plain > sql > json > table;
  `x` means expanded for table/json/sql.

#### Truncation marker
The immediate path caps display at 30 rows when >50 and currently warns on **stderr**
only, which vanishes when output is piped. For the **sql** format, when truncated, emit
the notice *into the output stream* as a SQL comment after the statements so captured
output is self-describing:
```
-- output truncated at 30 of 1234 rows;
-- use \re show format=sql to emit all rows
```
(`format=sql-expanded` in the message when expanded). query.rs owns this: after rendering,
if sql && truncated → write the comment to the writer instead of the stderr warning. plain
and the existing formats (table/expanded/json/csv) keep the current stderr warning
unchanged — json's pre-existing partial-array behaviour is intentionally left as-is and
out of scope here.
- `repl/result.rs` `format_result_using_display_module`: add Plain / Sql / SqlExpanded
  arms; derive table from `result.query` for the sql arms.

### Completion & help
- `completer/result.rs`: add `plain`, `sql`, `sql-expanded` to format completions.
- `completer/query_modifiers.rs`: add `p` and `s` to `MODIFIER_CHARS`; special-case the
  used-modifier parse loop so a trailing `s` followed by `et` is treated as the start of
  `set`, not the sql modifier.
- `repl/help.rs`: add rows for plain (`\gp`), sql (`\gs`), sql-expanded (`\gsx`).

### Tests
- query_modifiers: `\gp`, `\gs`, `\gsx`, `\gxs`; `\gset`/`\gxset` still parse as set.
- result parser: `format=plain|sql|sql-expanded`.
- completer: `\gp`/`\gs` suggested; `\gset` still suggested.
- table derivation: simple FROM, schema-qualified, alias, join (leftmost), subquery →
  None, no FROM → None.
- plain renderer (DB): single column, multi-column warning, nulls.
- sql renderer (DB): quoting (strings/quotes/numbers/bool/null), multi-row vs expanded,
  derived vs default table, column filtering.

### Generated docs
- run `./update-usage.sh` if help/usage output changed.

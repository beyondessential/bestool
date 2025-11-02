# psql-inspired PostgreSQL client

This tool was initially created by [BES](https://bes.au) as a series of augments
on top of the native psql client to increase our safety when interacting with
PostgreSQL databases, especially in sensitive environments. It has since been
rewritten entirely as a standalone tool, which maintains a similar syntax as
native psql, but with a different feature set and in some cases a different
syntax and behaviour.

The primary features are
- read/write modes, where a bestool-psql session will by default start in "read-only mode", which prohibits write operations;
- audit logging, which logs all queries and user information to a local database for auditing purposes (as well as providing cross-platform history);
- a simpler syntax and command set for variables;
- snippet management.

## Install

Available on [crates.io](https://crates.io/crates/bestool-psql):

```bash
cargo install bestool-psql
```

There are no binary builds available at this time.

The crate also exposes a library interface which embeds the tool in another CLI application.

## CLI Arguments

| Argument | Short | Type | Default | Description |
|----------|-------|------|---------|-------------|
| `CONNSTRING` | | `STRING` | required | Database name or connection string (e.g., 'mydb' or 'postgresql://user:password@localhost:5432/dbname') |
| `--write` | `-W` | `FLAG` | false | Enable write mode for this session. By default the session is read-only. To enable writes, pass this flag. This also disables autocommit, so you need to issue a COMMIT; command whenever you perform a write (insert, update, etc), as an extra safety measure. |
| `--theme` | | `STRING` | auto | The theme of your terminal (light, dark, or auto). 'auto' attempts to detect terminal background, defaults to 'dark' if detection fails. |
| `--audit-path` | | `PATH` | ~/.local/state/bestool-psql/history.redb | Path to audit database |

## Interactive Commands

All commands and some of the SQL has extensive tab completion, give it a try!

### Metacommands

| Command | Description |
|---------|-------------|
| `\help` or `\?` | Show a help page |
| `\q` or Ctrl-D | Quit |
| `\x` | Toggle expanded output mode |
| `\W` | Toggle write mode |
| `\e [query]` | Edit query in external editor |
| `\i <file> [var=val...]` | Execute commands from file |
| `\o [file]` | Send query results to file (or close if no file) |
| `\re list[+] [N]` | List the last N (default 20) saved results |
| `\re show [n=N] [format=FMT] [to=PATH] [cols=COLS] [limit=N] [offset=N]` | Display a saved result |
| `\snip run <name> [var=val...]` | Run a saved snippet |
| `\snip save <name>` | Save the preceding command as a snippet |
| `\set <name> <value>` | Set a variable |
| `\unset <name>` | Unset a variable |
| `\get <name>` | Print a variable value |
| `\vars [pattern]` | List variables (optionally matching pattern) |

### Database Exploration

| Command | Alias | Description |
|---------|-------|-------------|
| `\describe [pattern]` | `\d` | Describe database objects |
| `\list table [pattern]` | `\dt` | List tables |
| `\list view [pattern]` | `\dv` | List views |
| `\list function [pattern]` | `\df` | List functions |
| `\list index [pattern]` | `\di` | List indexes |
| `\list schema [pattern]` | `\dn` | List schemas |
| `\list sequence [pattern]` | `\ds` | List sequences |

There's also two modifiers you can add:
- `+` toggles "detailed" mode, which shows more information
- `!` toggles "same connection" mode; by default these commands are executed
  in a separate connection so they can be used to explore the database without
  affecting the current session. In some cases, you might want to execute them
  in the same connection as the session, so that you can see changes you've
  made to the database before committing them.

Look [in EXAMPLES.md](./EXAMPLES.md#database-exploration) for more.

### Query Modifiers

Query modifiers are used after a query to modify its execution behavior.

| Modifier | Description |
|----------|-------------|
| `\g` | Execute query |
| `\gx` | Execute query with expanded output |
| `\gj` | Execute query with JSON output |
| `\gv` | Execute query without variable interpolation ("verbatim") |
| `\go <file>` | Execute query and write output to file |
| `\gset [prefix]` | Execute query and store results in variables |

Modifiers can be combined, e.g. `\gxj` for expanded JSON output.

### Variable Interpolation

Variables can be used within SQL queries to dynamically substitute values.
Note this is a different syntax from native psql.

| Syntax | Description |
|--------|-------------|
| `${name}` | Replace with variable value (errors if not set) |
| `${{name}}` | Escape: produces `${name}` without replacement |

### Formats

| Format | Use with | Description |
|--------|----------|-------------|
| Table | `query \g` or `\re show format=table` | Default table format |
| Expanded | `query \gx` or `\re show format=expanded` | One table per result row |
| JSON | `query \gj` or `\re show format=json` | JSON lines: one row (as object) per line |
| Expanded JSON | `query \gxj` or `\re show format=json-pretty` | An array of objects (one per row), pretty-printed |
| CSV | `\re show format=csv` | Comma-separated values |
| Excel | `\re show format=excel to=<filename>` | XLSX-format spreadsheet |
| SQLite | `\re show format=sqlite to=<filename>` | SQLite database with a table named `results` |

## Usage

### Basic Connection

```bash
# Connect to local database
bestool-psql mydb

# Connect with full connection string
bestool-psql "postgresql://user:password@localhost:5432/mydb"
```

### Write Mode

```bash
# Enable write mode (remember to COMMIT!)
bestool-psql -W mydb
```

### Using Variables

```sql
-- Set a variable
\set table_name users

-- Use in query
SELECT * FROM ${table_name};

-- Escape variable syntax
SELECT '${{table_name}}' as literal_text;
```

### Snippets

```sql
-- Save current query as a snippet
\snip save my_query

-- Run the saved snippet later
\snip run my_query

-- Run snippet with variable override
\snip run my_query table_name=products
```

### Output Modes

```sql
-- Expanded output
SELECT * FROM users \gx;

-- JSON output
SELECT * FROM users \gj;

-- Pretty-printed JSON
SELECT * FROM users \gxj;

-- Write to file
SELECT * FROM users \go /tmp/results.txt;
```

### Executing from Files

```sql
-- Execute commands from a file
\i /path/to/script.sql

-- With variables (that only apply to this file execution)
\i /path/to/script.sql var1=value1 var2=value2
```

### Output everything to a file

```sql
\o /tmp/output.txt

-- This will print to the file instead of the screen
SELECT * FROM users;

-- This will print in json format to the file
SELECT * FROM users \gj;

-- Toggle off (now things will print to the screen again)
\o
```

Make sure to check out the [EXAMPLES.md](./EXAMPLES.md) for more.

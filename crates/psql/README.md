# psql2 - Async PostgreSQL Client

A modern, async PostgreSQL client built in Rust with an interactive REPL interface.

## CLI Arguments

| Argument | Short | Type | Default | Description |
|----------|-------|------|---------|-------------|
| `DBNAME` | | `STRING` | required | Database name or connection string (e.g., 'mydb' or 'postgresql://user:password@localhost:5432/dbname') |
| `--user` | `-U` | `STRING` | | Database user for tracking (defaults to $USER) |
| `--write` | `-W` | `FLAG` | false | Enable write mode for this session. By default the session is read-only. To enable writes, pass this flag. This also disables autocommit, so you need to issue a COMMIT; command whenever you perform a write (insert, update, etc), as an extra safety measure. |
| `--theme` | | `STRING` | auto | Syntax highlighting theme (light, dark, or auto). Controls the color scheme for SQL syntax highlighting in the input line. 'auto' attempts to detect terminal background, defaults to 'dark' if detection fails. |
| `--audit-path` | | `PATH` | ~/.local/state/bestool-psql/history.redb | Path to audit database |

## Interactive Commands

### Metacommands

| Command | Description |
|---------|-------------|
| `\?` | Show this help |
| `\help` | Show this help |
| `\q` | Quit |
| `\x` | Toggle expanded output mode |
| `\W` | Toggle write mode |
| `\e [query]` | Edit query in external editor |
| `\i <file> [var=val...]` | Execute commands from file |
| `\o [file]` | Send query results to file (or close if no file) |
| `\debug [cmd]` | Debug commands (run `\debug` for options) |
| `\snip run <name> [var=val...]` | Run a saved snippet |
| `\snip save <name>` | Save the preceding command as a snippet |
| `\set <name> <value>` | Set a variable |
| `\unset <name>` | Unset a variable |
| `\get <name>` | Get and print a variable value |
| `\vars [pattern]` | List variables (optionally matching pattern) |

### Query Modifiers

Query modifiers are used after a query to modify its execution behavior.

| Modifier | Description |
|----------|-------------|
| `\g` | Execute query |
| `\gx` | Execute query with expanded output |
| `\gj` | Execute query with JSON output |
| `\gv` | Execute query without variable interpolation |
| `\go <file>` | Execute query and write output to file |
| `\gset [prefix]` | Execute query and store results in variables |

Modifiers can be combined, e.g. `\gxj` for expanded JSON output.

### Variable Interpolation

Variables can be used within SQL queries to dynamically substitute values.

| Syntax | Description |
|--------|-------------|
| `${name}` | Replace with variable value (errors if not set) |
| `${{name}}` | Escape: produces `${name}` without replacement |

## Examples

### Basic Connection

```bash
# Connect to local database
psql2 mydb

# Connect with custom user
psql2 -U myuser mydb

# Connect with full connection string
psql2 "postgresql://user:password@localhost:5432/mydb"
```

### Write Mode

```bash
# Enable write mode (remember to COMMIT!)
psql2 -W mydb
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

### Running Snippets

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

-- Both expanded and JSON
SELECT * FROM users \gxj;

-- Write to file
SELECT * FROM users \go /tmp/results.txt;
```

### Executing from Files

```bash
# Execute commands from a file with variables
\i /path/to/script.sql var1=value1 var2=value2
```

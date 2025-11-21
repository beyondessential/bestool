# Command-Line Help for `bestool-psql`

This document contains the help content for the `bestool-psql` command-line program.

**Command Overview:**

* [`bestool-psql`↴](#bestool-psql)

## `bestool-psql`

Async PostgreSQL client

**Usage:** `bestool-psql [OPTIONS] [CONNSTRING]`

###### **Arguments:**

* `<CONNSTRING>` — Database name or connection URL

   Can be a simple database name (e.g., 'mydb') or full connection string (e.g., 'postgresql://user:password@localhost:5432/dbname')

###### **Options:**

* `--color <MODE>` — When to use terminal colours.

   You can also set the `NO_COLOR` environment variable to disable colours, or the `CLICOLOR_FORCE` environment variable to force colours. Defaults to `auto`, which checks whether the output is a terminal to decide.

  Default value: `auto`

  Possible values:
  - `auto`:
    Automatically detect whether to use colours
  - `always`:
    Always use colours, even if the terminal does not support them
  - `never`:
    Never use colours

* `-v`, `--verbose` — Set diagnostic log level.

   This enables diagnostic logging, which is useful for investigating bugs. Use multiple times to increase verbosity.

   You may want to use with `--log-file` to avoid polluting your terminal.

  Default value: `0`
* `--log-file <PATH>` — Write diagnostic logs to a file.

   This writes diagnostic logs to a file, instead of the terminal, in JSON format.

   If the path provided is a directory, a file will be created in that directory. The file name will be the current date and time, in the format `programname.YYYY-MM-DDTHH-MM-SSZ.log`.
* `--log-timeless` — Omit timestamps in logs.

   This can be useful when running under service managers that capture logs, to avoid having two timestamps. When run under systemd, this is automatically enabled.

   This option is ignored if the log file is set, or when using `RUST_LOG` or equivalent (as logging is initialized before arguments are parsed in that case); you may want to use `LOG_TIMELESS` instead in the latter case.
* `-W`, `--write` — Enable write mode for this session

   By default the session is read-only. To enable writes, pass this flag. This also disables autocommit, so you need to issue a COMMIT; command whenever you perform a write (insert, update, etc), as an extra safety measure.
* `--theme <THEME>` — Syntax highlighting theme (light, dark, or auto)

   Controls the color scheme for SQL syntax highlighting in the input line. 'auto' attempts to detect terminal background, defaults to 'dark' if detection fails.

  Default value: `auto`

  Possible values:
  - `light`
  - `dark`
  - `auto`:
    Auto-detect terminal theme

* `--audit-path <PATH>` — Path to audit database directory (default: /home/.local/state/bestool-psql)



<hr/>

<small><i>
    This document was generated automatically by
    <a href="https://crates.io/crates/clap-markdown"><code>clap-markdown</code></a>.
</i></small>


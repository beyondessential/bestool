# Command-Line Help for `bestool-alertd`

This document contains the help content for the `bestool-alertd` command-line program.

**Command Overview:**

* [`bestool-alertd`↴](#bestool-alertd)
* [`bestool-alertd run`↴](#bestool-alertd-run)
* [`bestool-alertd reload`↴](#bestool-alertd-reload)
* [`bestool-alertd loaded-alerts`↴](#bestool-alertd-loaded-alerts)
* [`bestool-alertd pause-alert`↴](#bestool-alertd-pause-alert)
* [`bestool-alertd validate`↴](#bestool-alertd-validate)

## `bestool-alertd`

BES tooling: Alert daemon

The daemon watches for changes to alert definition files and automatically reloads when changes are detected. You can also send SIGHUP to manually trigger a reload.

On Windows, the daemon can be installed as a native Windows service using the 'install' subcommand. See 'bestool-alertd install --help' for details.

The alert and target definitions are documented online at: <https://github.com/beyondessential/bestool/blob/main/crates/alertd/ALERTS.md> and <https://github.com/beyondessential/bestool/blob/main/crates/alertd/TARGETS.md>.

**Usage:** `bestool-alertd [OPTIONS] <COMMAND>`

###### **Subcommands:**

* `run` — Run the alert daemon
* `reload` — Send reload signal to running daemon
* `loaded-alerts` — List currently loaded alert files
* `pause-alert` — Temporarily pause an alert
* `validate` — Validate an alert definition file

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



## `bestool-alertd run`

Run the alert daemon

Starts the daemon which monitors alert definition files and executes alerts based on their configured schedules. The daemon will watch for file changes and automatically reload when definitions are modified.

**Usage:** `bestool-alertd run [OPTIONS]`

###### **Options:**

* `--database-url <DATABASE_URL>` — Database connection URL

   PostgreSQL connection URL, e.g., postgresql://user:pass@localhost/dbname
* `--glob <GLOB>` — Glob patterns for alert definitions

   Patterns can match directories (which will be read recursively) or individual files. Can be provided multiple times. Examples: /etc/tamanu/alerts, /opt/*/alerts, /etc/tamanu/alerts/**/*.yml
* `--email-from <EMAIL_FROM>` — Email sender address
* `--mailgun-api-key <MAILGUN_API_KEY>` — Mailgun API key
* `--mailgun-domain <MAILGUN_DOMAIN>` — Mailgun domain
* `--dry-run` — Execute all alerts once and quit (ignoring intervals)
* `--no-server` — Disable the HTTP server
* `--server-addr <SERVER_ADDR>` — HTTP server bind address(es)

   Can be provided multiple times. The server will attempt to bind to each address in order until one succeeds. Defaults to [::1]:8271 and 127.0.0.1:8271



## `bestool-alertd reload`

Send reload signal to running daemon

Connects to the running daemon's HTTP API and triggers a reload. This is an alternative to SIGHUP that works on all platforms including Windows.

**Usage:** `bestool-alertd reload [OPTIONS]`

###### **Options:**

* `--server-addr <SERVER_ADDR>` — HTTP server address(es) to try

   Can be provided multiple times. Will attempt to connect to each address in order until one succeeds. Defaults to [::1]:8271 and 127.0.0.1:8271



## `bestool-alertd loaded-alerts`

List currently loaded alert files

Connects to the running daemon's HTTP API and retrieves the list of currently loaded alert definition files.

**Usage:** `bestool-alertd loaded-alerts [OPTIONS]`

###### **Options:**

* `--server-addr <SERVER_ADDR>` — HTTP server address(es) to try

   Can be provided multiple times. Will attempt to connect to each address in order until one succeeds. Defaults to [::1]:8271 and 127.0.0.1:8271
* `--detail` — Show detailed state information for each alert



## `bestool-alertd pause-alert`

Temporarily pause an alert

Pauses an alert until the specified time. The alert will not execute during this period. The pause is lost when the daemon restarts.

**Usage:** `bestool-alertd pause-alert [OPTIONS] <ALERT>`

###### **Arguments:**

* `<ALERT>` — Alert file path to pause

###### **Options:**

* `--until <UNTIL>` — Time until which to pause the alert (fuzzy time format)

   Examples: "1 hour", "2 days", "next monday", "2024-12-25T10:00:00Z" Defaults to 1 week from now if not specified.
* `--server-addr <SERVER_ADDR>` — HTTP server address(es) to try

   Can be provided multiple times. Will attempt to connect to each address in order until one succeeds. Defaults to [::1]:8271 and 127.0.0.1:8271



## `bestool-alertd validate`

Validate an alert definition file

Parses an alert definition file and reports any syntax or validation errors. Uses pretty error reporting to pinpoint the exact location of problems. Requires the daemon to be running.

**Usage:** `bestool-alertd validate [OPTIONS] <FILE>`

###### **Arguments:**

* `<FILE>` — Path to the alert definition file to validate

###### **Options:**

* `--server-addr <SERVER_ADDR>` — HTTP server address(es) to try

   Can be provided multiple times. Will attempt to connect to each address in order until one succeeds. Defaults to [::1]:8271 and 127.0.0.1:8271



<hr/>

<small><i>
    This document was generated automatically by
    <a href="https://crates.io/crates/clap-markdown"><code>clap-markdown</code></a>.
</i></small>


# Usage

## Main Command

```
BES tooling: Alert daemon

The daemon watches for changes to alert definition files and automatically
reloads when changes are detected. You can also send SIGHUP to manually trigger
a reload.

On Windows, the daemon can be installed as a native Windows service using the
'install' subcommand. See 'bestool-alertd install --help' for details.

[1m[4mUsage:[0m [1mbestool-alertd[0m [OPTIONS] <COMMAND>

[1m[4mCommands:[0m
  [1mrun[0m            Run the alert daemon
  [1mreload[0m         Send reload signal to running daemon
  [1mloaded-alerts[0m  List currently loaded alert files
  [1mpause-alert[0m    Temporarily pause an alert
  [1mvalidate[0m       Validate an alert definition file
  [1mhelp[0m           Print this message or the help of the given subcommand(s)

[1m[4mOptions:[0m
      [1m--color[0m <MODE>
          When to use terminal colours.
          
          You can also set the `NO_COLOR` environment variable to disable
          colours, or the `CLICOLOR_FORCE` environment variable to force
          colours. Defaults to `auto`, which checks whether the output is a
          terminal to decide.

          Possible values:
          - [1mauto[0m:   Automatically detect whether to use colours
          - [1malways[0m: Always use colours, even if the terminal does not support
            them
          - [1mnever[0m:  Never use colours
          
          [default: auto]

  [1m-v[0m, [1m--verbose[0m...
          Set diagnostic log level.
          
          This enables diagnostic logging, which is useful for investigating
          bugs. Use multiple times to increase verbosity.
          
          You may want to use with `--log-file` to avoid polluting your
          terminal.

      [1m--log-file[0m [<PATH>]
          Write diagnostic logs to a file.
          
          This writes diagnostic logs to a file, instead of the terminal, in
          JSON format.
          
          If the path provided is a directory, a file will be created in that
          directory. The file name will be the current date and time, in the
          format `programname.YYYY-MM-DDTHH-MM-SSZ.log`.

      [1m--log-timeless[0m
          Omit timestamps in logs.
          
          This can be useful when running under service managers that capture
          logs, to avoid having two timestamps. When run under systemd, this is
          automatically enabled.
          
          This option is ignored if the log file is set, or when using
          `RUST_LOG` or equivalent (as logging is initialized before arguments
          are parsed in that case); you may want to use `LOG_TIMELESS` instead
          in the latter case.

  [1m-h[0m, [1m--help[0m
          Print help (see a summary with '-h')
```

## Subcommands

### `run`

```
Run the alert daemon

Starts the daemon which monitors alert definition files and executes alerts
based on their configured schedules. The daemon will watch for file changes and
automatically reload when definitions are modified.

[1m[4mUsage:[0m [1mbestool-alertd run[0m [OPTIONS]

[1m[4mOptions:[0m
      [1m--database-url[0m <DATABASE_URL>
          Database connection URL
          
          PostgreSQL connection URL, e.g.,
          postgresql://user:pass@localhost/dbname
          
          [env: DATABASE_URL=]

      [1m--glob[0m <GLOB>
          Glob patterns for alert definitions
          
          Patterns can match directories (which will be read recursively) or
          individual files. Can be provided multiple times. Examples:
          /etc/tamanu/alerts, /opt/*/alerts, /etc/tamanu/alerts/**/*.yml

      [1m--email-from[0m <EMAIL_FROM>
          Email sender address
          
          [env: EMAIL_FROM=]

      [1m--mailgun-api-key[0m <MAILGUN_API_KEY>
          Mailgun API key
          
          [env: MAILGUN_API_KEY=]

      [1m--mailgun-domain[0m <MAILGUN_DOMAIN>
          Mailgun domain
          
          [env: MAILGUN_DOMAIN=]

      [1m--dry-run[0m
          Execute all alerts once and quit (ignoring intervals)

      [1m--no-server[0m
          Disable the HTTP server

      [1m--server-addr[0m <SERVER_ADDR>
          HTTP server bind address(es)
          
          Can be provided multiple times. The server will attempt to bind to
          each address in order until one succeeds. Defaults to [::1]:8271 and
          127.0.0.1:8271

  [1m-h[0m, [1m--help[0m
          Print help (see a summary with '-h')
```

### `reload`

```
Send reload signal to running daemon

Connects to the running daemon's HTTP API and triggers a reload. This is an
alternative to SIGHUP that works on all platforms including Windows.

[1m[4mUsage:[0m [1mbestool-alertd reload[0m [OPTIONS]

[1m[4mOptions:[0m
      [1m--server-addr[0m <SERVER_ADDR>
          HTTP server address(es) to try
          
          Can be provided multiple times. Will attempt to connect to each
          address in order until one succeeds. Defaults to [::1]:8271 and
          127.0.0.1:8271

  [1m-h[0m, [1m--help[0m
          Print help (see a summary with '-h')
```

### `loaded-alerts`

```
List currently loaded alert files

Connects to the running daemon's HTTP API and retrieves the list of currently
loaded alert definition files.

[1m[4mUsage:[0m [1mbestool-alertd loaded-alerts[0m [OPTIONS]

[1m[4mOptions:[0m
      [1m--server-addr[0m <SERVER_ADDR>
          HTTP server address(es) to try
          
          Can be provided multiple times. Will attempt to connect to each
          address in order until one succeeds. Defaults to [::1]:8271 and
          127.0.0.1:8271

      [1m--detail[0m
          Show detailed state information for each alert

  [1m-h[0m, [1m--help[0m
          Print help (see a summary with '-h')
```

### `pause-alert`

```
Temporarily pause an alert

Pauses an alert until the specified time. The alert will not execute during this
period. The pause is lost when the daemon restarts.

[1m[4mUsage:[0m [1mbestool-alertd pause-alert[0m [OPTIONS] <ALERT>

[1m[4mArguments:[0m
  <ALERT>
          Alert file path to pause

[1m[4mOptions:[0m
      [1m--until[0m <UNTIL>
          Time until which to pause the alert (fuzzy time format)
          
          Examples: "1 hour", "2 days", "next monday", "2024-12-25T10:00:00Z"
          Defaults to 1 week from now if not specified.

      [1m--server-addr[0m <SERVER_ADDR>
          HTTP server address(es) to try
          
          Can be provided multiple times. Will attempt to connect to each
          address in order until one succeeds. Defaults to [::1]:8271 and
          127.0.0.1:8271

  [1m-h[0m, [1m--help[0m
          Print help (see a summary with '-h')
```

### `validate`

```
Validate an alert definition file

Parses an alert definition file and reports any syntax or validation errors.
Uses pretty error reporting to pinpoint the exact location of problems. Requires
the daemon to be running.

[1m[4mUsage:[0m [1mbestool-alertd validate[0m [OPTIONS] <FILE>

[1m[4mArguments:[0m
  <FILE>
          Path to the alert definition file to validate

[1m[4mOptions:[0m
      [1m--server-addr[0m <SERVER_ADDR>
          HTTP server address(es) to try
          
          Can be provided multiple times. Will attempt to connect to each
          address in order until one succeeds. Defaults to [::1]:8271 and
          127.0.0.1:8271

  [1m-h[0m, [1m--help[0m
          Print help (see a summary with '-h')
```


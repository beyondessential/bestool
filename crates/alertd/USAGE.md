# Usage

```
BES tooling: Alert daemon

The daemon watches for changes to alert definition files and automatically reloads when changes are
detected. You can also send SIGHUP to manually trigger a reload.

On Windows, the daemon can be installed as a native Windows service using the 'install' subcommand.
See 'bestool-alertd install --help' for details.

Usage: bestool-alertd [OPTIONS] <COMMAND>

Commands:
  run            Run the alert daemon
  reload         Send reload signal to running daemon
  loaded-alerts  List currently loaded alert files
  pause-alert    Temporarily pause an alert
  validate       Validate an alert definition file
  help           Print this message or the help of the given subcommand(s)

Options:
      --color <MODE>
          When to use terminal colours.
          
          You can also set the `NO_COLOR` environment variable to disable colours, or the
          `CLICOLOR_FORCE` environment variable to force colours. Defaults to `auto`, which checks
          whether the output is a terminal to decide.

          Possible values:
          - auto:   Automatically detect whether to use colours
          - always: Always use colours, even if the terminal does not support them
          - never:  Never use colours
          
          [default: auto]

  -v, --verbose...
          Set diagnostic log level.
          
          This enables diagnostic logging, which is useful for investigating bugs. Use multiple
          times to increase verbosity.
          
          You may want to use with `--log-file` to avoid polluting your terminal.

      --log-file [<PATH>]
          Write diagnostic logs to a file.
          
          This writes diagnostic logs to a file, instead of the terminal, in JSON format.
          
          If the path provided is a directory, a file will be created in that directory. The file
          name will be the current date and time, in the format
          `programname.YYYY-MM-DDTHH-MM-SSZ.log`.

      --log-timeless
          Omit timestamps in logs.
          
          This can be useful when running under service managers that capture logs, to avoid having
          two timestamps. When run under systemd, this is automatically enabled.
          
          This option is ignored if the log file is set, or when using `RUST_LOG` or equivalent (as
          logging is initialized before arguments are parsed in that case); you may want to use
          `LOG_TIMELESS` instead in the latter case.

  -h, --help
          Print help (see a summary with '-h')
```

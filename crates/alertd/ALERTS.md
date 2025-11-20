# Alert Definitions

Alerts are the core component of Alertd, representing conditions that need to be monitored and
notified about. Alerts are a YAML file, with a single document in the file. The absolute path to
the file is considered its identifier: identical contents at a different path will dupe the alert.

## Structure

```yaml
# Optional fields (with defaults)
enabled: true | false                    # Default: true
interval: <duration>                     # Default: "1 minute"
always-send: <always-send-config>        # Default: false
when-changed: <when-changed-config>      # Default: false

# Required: At least one source
sql: <string>                            # SQL query source
# OR
shell: <string>                          # Shell command
run: <string>                            # Command to run
# OR
event: <event-type>                      # Event-based trigger

# Optional: SQL-specific
numerical:                               # List of numerical thresholds
  - field: <string>
    alert-at: <number>
    clear-at: <number>                   # Optional

# Required: Targets
send:                                    # List of send targets
  - id: <string>
    subject: <string>                    # Optional
    template: <string>
```

## Duration Format

Formats accepted by `interval` and `always-send.after`:
- `<number> second(s)`
- `<number> minute(s)`
- `<number> hour(s)`
- `<number> day(s)`
- `<number>s`, `<number>m`, `<number>h`, `<number>d`

Examples: `30 seconds`, `5 minutes`, `2 hours`, `1 day`, `30s`, `5m`


## Source Types

### SQL Source

```yaml
sql: <query-string>
numerical:                               # Optional
  - field: <column-name>
    alert-at: <threshold>
    clear-at: <threshold>                # Optional
```

Context variables:
- `rows`: Array of query result rows (each row is a dict)
- `triggered`: Boolean indicating if previously triggered
- Standard variables (see below)

Numerical thresholds:
- Alert triggers when `field >= alert-at`
- Alert clears when `field <= clear-at` (if specified)
- If `clear-at` omitted, alert never auto-clears

### Shell Source

```yaml
shell: <shell-name>      # e.g., "bash", "sh", "python"
run: <command>           # Command to execute
```

Context variables:
- `output`: Command stdout as string
- `exit_code`: Process exit code
- `triggered`: Boolean indicating if previously triggered
- Standard variables (see below)

Alert triggers if exit code is not 0.

### Event Source

```yaml
event: <event-type>
```

Event types:
- `http`: HTTP POST to `/alert` endpoint
- `definition-error`: Alert definition file has errors
- `source-error`: Alert source execution failed

Event-specific context variables vary by type:
- `http`: `message`, `subject`, any other variables sent to the endpoint
- `definition-error`: `alert_file`, `error_message`
- `source-error`: `alert_file`, `error_message`

All `event` sources are internally defaulted to send to the `default` target with a very basic
template if none is defined through the alert files.

## Send Targets

### Simple Format

```yaml
send:
  - id: <target-id>              # References external target in _targets.yml
    subject: <template-string>   # Optional, Tera template
    template: <template-string>  # Required, Tera template (Markdown)
```

## Template Context

Standard variables available in all templates:
- `alert_file`: Path to alert definition file
- `filename`: Basename of alert file
- `hostname`: System hostname
- `now`: Current timestamp

Source-specific variables are added based on source type (see above).

## Template Syntax

Templates use [Tera](https://keats.github.io/tera/docs/) syntax:
- Variables: `{{ variable }}`
- Conditionals: `{% if condition %}...{% endif %}`
- Loops: `{% for item in items %}...{% endfor %}`
- Filters: `{{ value | filter }}`

Templates are rendered as Markdown and converted to HTML for email. Markdown will also pass through
HTML if you want to use HTML in your templates directly.

The Tera syntax is Jinja2-like, but with some differences! Check the docs, and don't forget to
`validate` the alert definition.

## Always-Send Config

By default, the "triggered" state of alerts is tracked internally. When an alert is triggered, it
is sent immediately to its targets. Subsequent checks that would trigger the alert are ignored,
until the alert goes back to "okay" state, at which point its triggered state is reset and the next
trigger will be sent to targets.

When `always-send: true`, the alert will be sent to its targets every time it is triggered, regardless of its previous state.

When `always-send: false`, the alert will only be sent to its targets if it is triggered for the first time or if it has been cleared.

There's an advanced configuration with `always-send.after` that allows you to resend alerts after a certain duration.

```yaml
# Simple boolean
always-send: true | false

# Timed resending
always-send:
  after: <duration>    # Resend after this duration
```

## When-Changed Config

By default, for SQL sources the alert is considered triggered if it returns any rows, and Shell
sources are considered triggered if they return a non-zero exit code. With `when-changed: true`,
the output (in rows or string) of the alert is kept track of, and the alert is considered
triggered if that output changes.

For SQL sources, the `except` and `only` fields (only one of them, you can't use both at once) can
be used to filter the fields to compare. For example, this can be used to allow datetimes to change
in the query without triggering the alert, but still being able to use them in the email template.

```yaml
# Simple boolean
when-changed: true | false

# Detailed configuration
when-changed:
  except: [<field>, ...]   # Exclude these fields from comparison
  only: [<field>, ...]     # Only compare these fields
```

Notes:
- `except` and `only` are mutually exclusive
- Fields refer to column names in SQL results or keys in context

## Examples

### SQL Alert

```yaml
interval: 5 minutes

sql: "SELECT count(*) as count FROM users WHERE created_at > NOW() - INTERVAL '1 hour'"

numerical:
  - field: count
    alert-at: 100
    clear-at: 50

send:
  - id: ops-team
    subject: "High user registration rate"
    template: |
      Alert: {{ rows[0].count }} users registered in the last hour.
```

### Shell Alert

```yaml
interval: 1 minute

shell: bash
run: "df -h / | tail -1 | awk '{print $5}' | sed 's/%//'"

send:
  - id: ops-team
    subject: "Disk space alert on {{ hostname }}"
    template: |
      Disk usage: {{ output }}%
```

### Event Alert

```yaml
event: http

send:
  - id: ops-team
    subject: "{{ subject | default(value='HTTP Alert') }}"
    template: |
      {{ message }}

      {% if custom %}
      Additional data: {{ custom | json_encode(pretty=true) }}
      {% endif %}
```

### Timed Resending

```yaml
sql: "SELECT 1 WHERE (SELECT pg_is_in_recovery()) = true"

always-send:
  after: 8 hours

send:
  - id: dba-team
    subject: "Database in recovery mode"
    template: "The database is still in recovery mode."
```

### Change Detection

```yaml
sql: "SELECT version, deployed_at FROM app_version ORDER BY deployed_at DESC LIMIT 1"

when-changed:
  only: [version]

send:
  - id: dev-team
    subject: "New deployment detected"
    template: |
      {% for row in rows %}
      - Version {{ row.version }} deployed at {{ row.deployed_at }}
      {% endfor %}
```

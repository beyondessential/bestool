# Send Targets

There must be at least one `_targets.yml` or `_targets.yaml` file in the directories that alertd
scans for alert definitions, and one of these files must contain at least one target. It's
recommended to have a target with `id: default`. If an explicit default isn't defined, the first
target in alphabetical ID order will be used as default.

## Target Types

### Email

Email targets send alerts via Mailgun. They require the `addresses` field.

```yaml
targets:
  - id: <unique-id>
    addresses:
      - <email-address>
      - <email-address>
```

### Slack

Slack targets post alerts to a Slack incoming webhook. They require the `webhook` field and
optionally accept a `fields` list to customize the JSON payload sent to the webhook.

```yaml
targets:
  - id: <unique-id>
    webhook: <slack-webhook-url>
    fields:                          # Optional, defaults shown below
      - name: hostname
        field: hostname
      - name: filename
        field: filename
      - name: subject
        field: subject
      - name: message
        field: body
```

Each field entry is either:
- **Template field**: `{ name: <key>, field: <template-field> }` — rendered from template context.
  Valid template fields: `hostname`, `filename`, `subject`, `body`, `interval`.
- **Fixed value**: `{ name: <key>, value: <string> }` — a static string.

If `fields` is omitted, the default set (`hostname`, `filename`, `subject`, `message`) is used.

## Structure

The target type is determined automatically by the fields present:
- If `addresses` is present, it's an email target.
- If `webhook` is present, it's a Slack target.

Targets of different types can be mixed freely in the same `_targets.yml` file, and multiple
targets of different types can share the same `id` to send alerts to both simultaneously.

## Examples

### Single Email Target

```yaml
targets:
  - id: ops-team
    addresses:
      - ops@example.com
```

### Single Slack Target

```yaml
targets:
  - id: ops-team
    webhook: https://hooks.example.com/services/T00000000/B00000000/XXXXXXXXXXXXXXXXXXXXXXXX
```

### Mixed Email and Slack

Alerts sent to `ops-team` will be delivered to both email and Slack:

```yaml
targets:
  - id: ops-team
    addresses:
      - ops@example.com
      - oncall@example.com

  - id: ops-team
    webhook: https://hooks.example.com/services/T00000000/B00000000/XXXXXXXXXXXXXXXXXXXXXXXX
```

### Slack with Custom Fields

```yaml
targets:
  - id: monitoring
    webhook: https://hooks.example.com/services/T00000000/B00000000/XXXXXXXXXXXXXXXXXXXXXXXX
    fields:
      - name: text
        field: body
      - name: server
        field: hostname
      - name: environment
        value: production
```

### Multiple Targets

```yaml
targets:
  - id: ops-team
    addresses:
      - ops@example.com
      - oncall@example.com
      - alerts@example.com

  - id: dev-team
    addresses:
      - developers@example.com

  - id: slack-alerts
    webhook: https://hooks.example.com/services/T00000000/B00000000/XXXXXXXXXXXXXXXXXXXXXXXX
```

### Multiple Files

Targets can be split across multiple `_targets.yml` files:

```
/etc/alertd/
├── alerts/
│   ├── disk-space.yml
│   └── database.yml
├── teams/
│   ├── _targets.yml        # ops-team, security-team
│   └── alerts/
│       └── security.yml
└── _targets.yml             # dev-team, dba-team
```

All `_targets.yml` files are merged together. Targets with the same ID are grouped: an alert
referencing that ID will be sent to all of them.

## Usage in Alerts

Reference targets in alert definitions:

```yaml
# In an alert definition file
sql: "SELECT 1"

send:
  - id: ops-team              # References the target defined in _targets.yml
    subject: "Alert"
    template: "Message"
```

Multiple alerts can reference the same target:

```yaml
# alert1.yml
send:
  - id: ops-team
    subject: "Alert 1"
    template: "..."

# alert2.yml
send:
  - id: ops-team
    subject: "Alert 2"
    template: "..."
```

Alerts can send to multiple targets:

```yaml
send:
  - id: ops-team
    subject: "Ops Alert"
    template: "..."

  - id: dev-team
    subject: "Dev Alert"
    template: "..."
```

## Email Configuration

Email sending requires Mailgun configuration provided via:
- Tamanu config files (when using `bestool tamanu alertd`)
- Environment variables or command-line options (when using standalone `alertd`)

The `_targets.yml` file only defines recipients, not email server configuration.

## Slack Configuration

Slack targets are self-contained: the webhook URL in `_targets.yml` is all that's needed. No
additional daemon configuration is required. To obtain a webhook URL, create an [Incoming
Webhook](https://api.slack.com/messaging/webhooks) in your Slack workspace.
# Send Targets

There must be at least one `_targets.yml` or `_targets.yaml` file in the directories that alertd
scans for alert definitions, and one of these files must contain at least one target. It's
recommended to have a target with `id: default`. If an explicit default isn't defined, the first
target in alphabetical ID order will be used as default.

The one exception: if there are no `_targets.yml` files at all *and* the canopy auth path is
available (either tailscale reachability or an mTLS device key), alertd synthesises a canopy
target as the default automatically — see [Canopy](#canopy) below.

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

### Canopy

Canopy targets push events to a [canopy](https://meta.tamanu.app) server's `/events` API. Canopy
aggregates pushed events into deduplicated *issues* keyed by `source` + `ref`; alertd posts an
`active: true` event when an alert triggers and an `active: false` event when it clears, so issues
auto-resolve.

```yaml
targets:
  - id: <unique-id>
    canopy:
      url: https://meta.tamanu.app    # Optional, default shown
      source: <string>                # Required, identifies this device's event stream
      severity: <severity>            # Optional, default: error
```

`severity` is one of the RFC 5424 levels (lowercase): `emergency`, `alert`, `critical`, `error`,
`warning`, `notice`, `info`, `debug`. Canopy treats events at or above `error` as incident-grade.

#### Authentication

Canopy requires one of two auth paths; alertd probes them at startup and on every reload:

1. **Tailscale** — if `https://tamanu-meta-prod.tail53aef.ts.net/public/events` is reachable
   from this host (i.e. the host is on the canopy tailnet), events are pushed there without any
   client cert. This path is preferred when available.

2. **mTLS via Tamanu device key** — falls back to the public endpoint (`url:` above) using a
   self-signed client cert minted from a Tamanu device key. The key is the value stored in the
   Tamanu DB at `local_system_facts(key='deviceKey')`. The certificate has a 6-day validity and
   is automatically renewed every 5 days while the daemon is running.

   `bestool tamanu alertd` fetches the device key from the Tamanu DB automatically.
   Standalone `bestool-alertd run` takes the key path via `--device-key-file <PATH>` (env
   `DEVICE_KEY_FILE`).

#### Synthesised default

If no `_targets.yml` file is present anywhere in the scanned directories *and* one of the
canopy auth paths is available, alertd synthesises a canopy default target with:

- `source: bestool-alertd`
- `url`: the default canopy URL (ignored when tailscale is the active path)
- `severity: error`

This lets event sources like `database-down` go somewhere visible without any configuration on
hosts that already have canopy auth set up. To get any other behaviour — different source,
non-canopy fallback, etc. — add a real `_targets.yml`.

## Structure

The target type is determined automatically by the fields present:
- If `addresses` is present, it's an email target.
- If `webhook` is present, it's a Slack target.
- If `canopy:` (a nested object) is present, it's a canopy target.

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

## Canopy Configuration

Canopy targets pick up auth from the environment (tailscale presence or device key) — see the
[Canopy](#canopy) target type section above. No `_targets.yml` field configures the auth path.

The event `ref` (canopy's dedup key) is auto-derived as `{hostname}/{alert-stem}:{target-id}`,
so the same alert firing on different hosts or to different canopy targets produces distinct
canopy issues.
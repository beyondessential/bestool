# Send Targets

There must be at least one `_targets.yml` or `_targets.yaml` file in the directories that alertd
scans for alert definitions, and one of these files must contain at least one target. It's
recommended to have a target with `id: default`. If an explicit default isn't defined, the first
target in alphabetical ID order will be used as default.

## Structure

```yaml
targets:
  - id: <unique-id>
    addresses:
      - <email-address>
      - <email-address>
      # ... more addresses
  # ... more targets
```

## Examples

### Single Target

```yaml
targets:
  - id: ops-team
    addresses:
      - ops@example.com
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

  - id: dba-team
    addresses:
      - dba@example.com
      - database-alerts@example.com
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

All `_targets.yml` files are merged together. IDs must be unique across all files.

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

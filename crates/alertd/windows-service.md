# Windows Service Integration

The alertd daemon can be installed and run as a native Windows service, allowing it to start automatically with Windows and be managed through the standard Windows service management tools.

## Installation

To install alertd as a Windows service, run:

```powershell
bestool-alertd.exe install
```

This creates a service named `bestool-alertd` with the display name "BES Alert Daemon".

## Configuration

The service requires configuration through environment variables. You can set these using the Windows registry or through PowerShell:

### Using PowerShell (requires administrator privileges):

```powershell
# Set environment variables for the service
$serviceName = "bestool-alertd"
$regPath = "HKLM:\SYSTEM\CurrentControlSet\Services\$serviceName"

# Database connection
reg add "$regPath" /v Environment /t REG_MULTI_SZ /d "DATABASE_URL=postgresql://user:password@localhost/tamanu_meta" /f

# Alert definition globs (use registry for complex configurations)
# Alternatively, modify the service command line arguments
sc config bestool-alertd binPath= "C:\path\to\bestool-alertd.exe service --glob C:\alerts\*.yml --database-url postgresql://user:password@localhost/tamanu_meta"
```

### Required Configuration:

- `--database-url` or `DATABASE_URL`: PostgreSQL connection string
- `--glob`: One or more glob patterns for alert definition files

### Optional Configuration:

- `--email-from` or `EMAIL_FROM`: Email sender address
- `--mailgun-api-key` or `MAILGUN_API_KEY`: Mailgun API key
- `--mailgun-domain` or `MAILGUN_DOMAIN`: Mailgun domain
- `--no-server`: Disable the HTTP server (port 8271)

## Managing the Service

### Start the service:
```powershell
Start-Service bestool-alertd
# or
sc start bestool-alertd
```

### Stop the service:
```powershell
Stop-Service bestool-alertd
# or
sc stop bestool-alertd
```

### Check service status:
```powershell
Get-Service bestool-alertd
# or
sc query bestool-alertd
```

### View service logs:
Service logs are written to the Windows Event Log. View them using Event Viewer or PowerShell:

```powershell
Get-EventLog -LogName Application -Source bestool-alertd -Newest 50
```

## Reloading Configuration

Even when running as a service, you can trigger a configuration reload without restarting:

```powershell
bestool-alertd.exe --reload
```

This connects to the HTTP API (default port 8271) and triggers a reload of alert definitions.

## Uninstallation

To remove the service:

```powershell
# Stop the service first
Stop-Service bestool-alertd

# Uninstall
bestool-alertd.exe uninstall
```

## Service Command Line Example

A complete service installation with all options:

```powershell
# Install service
bestool-alertd.exe install

# Configure service with full command line
sc config bestool-alertd binPath= "\"C:\Program Files\BES\bestool-alertd.exe\" service --glob \"C:\tamanu\alerts\**\*.yml\" --glob \"C:\tamanu\alerts\*.yaml\" --database-url \"postgresql://tamanu:password@localhost/tamanu_meta\" --email-from \"alerts@example.com\""

# Set environment variables for sensitive data
# (Recommended for API keys and passwords)
```

## Troubleshooting

### Service fails to start
- Check Event Viewer for error messages
- Verify database connection string is correct
- Ensure glob patterns point to valid directories
- Verify the service user has permissions to read alert files

### Service starts but doesn't execute alerts
- Check that alert definition files are valid YAML
- Verify database connectivity from the service user context
- Review logs in Event Viewer for specific errors
- Ensure the HTTP server port (8271) is not blocked by firewall

### Cannot reload configuration
- Verify the HTTP server is enabled (don't use `--no-server`)
- Check that port 8271 is accessible locally
- Ensure the service is running
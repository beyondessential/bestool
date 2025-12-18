use std::str::FromStr;

use miette::{IntoDiagnostic, Result, WrapErr};
use mobc_postgres::tokio_postgres;
use tokio_postgres::Config;
use tracing::debug;

/// Parse a connection URL and handle Unix socket paths properly
pub fn parse_connection_url(url: &str) -> Result<Config> {
	debug!("Parsing connection URL");

	// First, try standard parsing
	let mut config = Config::from_str(url)
		.into_diagnostic()
		.wrap_err("parsing connection string")?;

	debug!("Initial hosts: {:?}", config.get_hosts());
	debug!("Initial SSL mode: {:?}", config.get_ssl_mode());

	// Check if we need to handle Unix socket paths
	config = handle_unix_sockets(config, url)?;

	debug!("Final hosts: {:?}", config.get_hosts());
	debug!("Final SSL mode: {:?}", config.get_ssl_mode());

	Ok(config)
}

/// Handle Unix socket paths in the configuration
#[allow(unused_variables)]
fn handle_unix_sockets(mut config: Config, original_url: &str) -> Result<Config> {
	// Check if any of the configured hosts look like Unix socket paths
	let hosts: Vec<_> = config.get_hosts().to_vec();

	#[cfg(unix)]
	{
		use std::path::Path;
		let mut is_unix_socket = false;

		if hosts.is_empty() {
			// No host specified - try to detect default PostgreSQL socket location
			if let Some(socket_dir) = detect_default_postgres_socket() {
				config.host_path(&socket_dir);
				is_unix_socket = true;
			}

			if !is_unix_socket {
				// Fall back to localhost if we can't find a socket
				config.host("localhost");
			}
		}

		if let Some(tokio_postgres::config::Host::Unix(_)) = hosts.first() {
			// Already configured as Unix socket
			is_unix_socket = true;
		}

		if let Some(tokio_postgres::config::Host::Tcp(hostname)) = hosts.first() {
			if hostname.starts_with('/') {
				// It's a path string but was parsed as TCP host
				// Rebuild config with proper Unix socket path
				let socket_path = Path::new(hostname);
				config.host_path(socket_path);
				is_unix_socket = true;
			}

			if !is_unix_socket
				&& let Some(extracted_host) = extract_host_from_url(original_url)
				&& extracted_host.starts_with('/')
			{
				// Special case: URL encoding might have mangled the path
				// The original URL had a path, but it got parsed as TCP
				let socket_path = Path::new(&extracted_host);
				config.host_path(socket_path);
				is_unix_socket = true;
			}
		}

		// Disable SSL for Unix socket connections
		if is_unix_socket {
			config.ssl_mode(tokio_postgres::config::SslMode::Disable);
		}
	}

	#[cfg(not(unix))]
	{
		// On non-Unix systems, always use localhost if no host specified
		if hosts.is_empty() {
			config.host("localhost");
		}
	}

	Ok(config)
}

/// Extract the host part from a PostgreSQL connection URL
#[cfg(unix)]
fn extract_host_from_url(url: &str) -> Option<String> {
	// Handle postgresql:// or postgres:// schemes
	let url = url
		.strip_prefix("postgresql://")
		.or_else(|| url.strip_prefix("postgres://"))?;

	// Skip past credentials if present (username:password@)
	let after_credentials = if let Some(at_pos) = url.find('@') {
		&url[at_pos + 1..]
	} else {
		url
	};

	// Extract host (up to / or : for port, whichever comes first)
	let host_end = after_credentials
		.find('/')
		.into_iter()
		.chain(after_credentials.find(':'))
		.min()
		.unwrap_or(after_credentials.len());

	let host = &after_credentials[..host_end];

	if host.is_empty() {
		None
	} else {
		Some(
			percent_encoding::percent_decode_str(host)
				.decode_utf8()
				.ok()?
				.to_string(),
		)
	}
}

/// Detect the default PostgreSQL Unix socket directory on the system
#[cfg(unix)]
fn detect_default_postgres_socket() -> Option<std::path::PathBuf> {
	use std::path::Path;

	// Common PostgreSQL Unix socket locations, in order of preference
	let candidates = [
		"/var/run/postgresql",
		"/tmp",
		"/var/run",
		"/usr/local/var/run/postgresql",
	];

	for candidate in candidates {
		let path = Path::new(candidate);
		if path.exists() && path.is_dir() {
			// Check if there's a PostgreSQL socket file here
			if let Ok(entries) = std::fs::read_dir(path) {
				for entry in entries.flatten() {
					let file_name = entry.file_name();
					let file_name_str = file_name.to_string_lossy();
					if file_name_str.starts_with(".s.PGSQL.") {
						return Some(path.to_path_buf());
					}
				}
			}
			// Even if no socket file found yet, this directory exists and is valid
			// PostgreSQL might be starting up
			return Some(path.to_path_buf());
		}
	}

	None
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_extract_host_from_url_with_tcp_host() {
		let url = "postgresql://user:pass@localhost:5432/dbname";
		let host = extract_host_from_url(url);
		assert_eq!(host, Some("localhost".to_string()));
	}

	#[test]
	fn test_extract_host_from_url_with_unix_socket() {
		// Unix socket paths need to be percent-encoded in URLs
		let url = "postgresql://user:pass@%2Fvar%2Frun%2Fpostgresql:5432/dbname";
		let host = extract_host_from_url(url);
		assert_eq!(host, Some("/var/run/postgresql".to_string()));
	}

	#[test]
	fn test_extract_host_from_url_with_encoded_path() {
		let url = "postgresql://user:pass@%2Fvar%2Frun%2Fpostgresql/dbname";
		let host = extract_host_from_url(url);
		assert_eq!(host, Some("/var/run/postgresql".to_string()));
	}

	#[test]
	fn test_extract_host_from_url_no_credentials() {
		let url = "postgresql://localhost/dbname";
		let host = extract_host_from_url(url);
		assert_eq!(host, Some("localhost".to_string()));
	}

	#[test]
	fn test_extract_host_from_url_with_port() {
		let url = "postgresql://localhost:5433/dbname";
		let host = extract_host_from_url(url);
		assert_eq!(host, Some("localhost".to_string()));
	}

	#[test]
	fn test_handle_unix_sockets_with_path_string() {
		let config = Config::new();
		let mut config_with_host = config.clone();
		config_with_host.host("/var/run/postgresql");

		let result = handle_unix_sockets(config_with_host, "postgresql:///dbname");
		assert!(result.is_ok());
	}

	#[test]
	fn test_handle_unix_sockets_empty_host() {
		let config = Config::new();
		let result = handle_unix_sockets(config, "postgresql:///dbname");
		assert!(result.is_ok());
		// Should either set a Unix socket path or localhost
		let config = result.unwrap();
		assert!(!config.get_hosts().is_empty());
	}

	#[test]
	#[cfg(unix)]
	fn test_detect_default_postgres_socket() {
		// This test checks if the function can find a valid directory
		let result = detect_default_postgres_socket();
		// We can't assert it exists since it depends on the system,
		// but we can verify the function runs without panicking
		if let Some(path) = result {
			assert!(path.is_absolute());
		}
	}
}

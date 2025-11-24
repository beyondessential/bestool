use std::time::Duration;

use miette::{IntoDiagnostic, Report, Result, WrapErr, miette};
use mobc::{Connection, Pool};
use tokio_postgres::config::SslMode;
use tracing::debug;

pub use manager::{PgConnectionManager, PgError};

mod manager;
mod tls;
mod url;

/// Check if an error is a TLS/SSL error
fn is_tls_error(error: &Report) -> bool {
	if error.downcast_ref::<rustls::Error>().is_some() {
		return true;
	}

	// Check the error chain for PgError::Tls
	let mut source = error.source();
	while let Some(err) = source {
		if err.downcast_ref::<rustls::Error>().is_some() {
			return true;
		}
		source = err.source();
	}

	let message = error.to_string();
	message.contains("tls:")
		|| message.contains("rustls")
		|| message.contains("certificate")
		|| message.contains("TLS handshake")
		|| message.contains("invalid configuration")
}

/// Check if an error is an authentication error
fn is_auth_error(error: &Report) -> bool {
	if let Some(db_error) = error.downcast_ref::<tokio_postgres::Error>()
		&& let Some(db_error) = db_error.as_db_error()
	{
		// PostgreSQL error codes for authentication failures:
		// 28000 - invalid_authorization_specification
		// 28P01 - invalid_password
		let code = db_error.code().code();
		return code == "28000" || code == "28P01";
	}

	// Check for other connection errors that might indicate auth issues
	let message = error.to_string();
	message.contains("password authentication failed")
		|| message.contains("no password supplied")
		|| message.contains("authentication failed")
}

pub type PgConnection = Connection<manager::PgConnectionManager>;

#[derive(Debug, Clone)]
pub struct PgPool {
	pub manager: manager::PgConnectionManager,
	pub inner: Pool<manager::PgConnectionManager>,
}

impl PgPool {
	/// Returns a single connection by either opening a new connection
	/// or returning an existing connection from the connection pool. Conn will
	/// block until either a connection is returned or timeout.
	pub async fn get(&self) -> Result<PgConnection, mobc::Error<PgError>> {
		self.inner.get().await
	}

	/// Retrieves a connection from the pool, waiting for at most `timeout`
	///
	/// The given timeout will be used instead of the configured connection
	/// timeout.
	pub async fn get_timeout(
		&self,
		duration: Duration,
	) -> Result<PgConnection, mobc::Error<PgError>> {
		self.inner.get_timeout(duration).await
	}
}

/// Create a connection pool from a connection URL
///
/// Supports Unix socket connections via:
/// - Query parameter: `postgresql:///dbname?host=/var/run/postgresql`
/// - Percent-encoded host: `postgresql://%2Fvar%2Frun%2Fpostgresql/dbname`
/// - Empty host (auto-detects Unix socket or falls back to localhost): `postgresql:///dbname`
///
/// Unix socket connections automatically disable SSL/TLS.
///
/// # Password Prompting
///
/// If the connection fails with an authentication error and no password was provided
/// in the connection URL, the function will prompt the user to enter a password
/// interactively. The password will be read securely without echoing to the terminal.
pub async fn create_pool(url: &str, application_name: &str) -> Result<PgPool> {
	let mut config = url::parse_connection_url(url)?;

	config.application_name(application_name);

	let mut tried_ssl_fallback = false;

	// Try to connect, and if it fails with auth error, prompt for password
	let pool = loop {
		debug!("Creating manager");
		let tls = config.get_ssl_mode() != SslMode::Disable;
		let manager = crate::pool::PgConnectionManager::new(config.clone(), tls);

		debug!("Creating pool");
		let pool = Pool::builder()
			.max_lifetime(Some(Duration::from_secs(3600)))
			.build(manager.clone());

		let pool = PgPool {
			manager,
			inner: pool,
		};

		debug!("Checking pool");
		match check_pool(&pool).await {
			Ok(_) => {
				if tried_ssl_fallback {
					tracing::info!("Connected successfully with SSL disabled after TLS error");
				}
				break pool;
			}
			Err(e) => {
				debug!("Connection error: {:#}", e);
				debug!(
					"is_tls_error: {}, is_auth_error: {}",
					is_tls_error(&e),
					is_auth_error(&e)
				);

				if is_tls_error(&e) {
					// If SSL mode is prefer and we haven't tried fallback yet, retry with SSL disabled
					if config.get_ssl_mode() == SslMode::Prefer && !tried_ssl_fallback {
						debug!("TLS failed with prefer mode, retrying with SSL disabled");
						config.ssl_mode(SslMode::Disable);
						tried_ssl_fallback = true;
						continue;
					}

					// TLS error - suggest disabling SSL
					return Err(e).wrap_err(
						"TLS/SSL connection failed. Try using --ssl disable, \
						or use a connection URL with sslmode=disable: \
						postgresql://user@host/db?sslmode=disable",
					);
				} else if is_auth_error(&e) && config.get_password().is_none() {
					let password = rpassword::prompt_password("Password: ").into_diagnostic()?;
					config.password(password);
					// Loop will retry with the new password
				} else {
					// Not an auth error or we already have a password, re-throw
					return Err(e);
				}
			}
		}
	};

	Ok(pool)
}

/// Check if we can actually establish a connection
async fn check_pool(pool: &PgPool) -> Result<()> {
	let conn = match pool.get().await {
		Err(mobc::Error::Inner(db_err)) => {
			return Err(match db_err.as_db_error() {
				Some(db_err) => miette!(
					"E{code} at {func} in {file}:{line}",
					code = db_err.code().code(),
					func = db_err.routine().unwrap_or("{unknown}"),
					file = db_err.file().unwrap_or("unknown.c"),
					line = db_err.line().unwrap_or(0)
				),
				_ => miette!("{db_err}"),
			})
			.wrap_err(
				db_err
					.as_db_error()
					.map(|e| e.to_string())
					.unwrap_or_default(),
			)?;
		}
		res @ Err(_) => {
			let res = res.map(drop).into_diagnostic();
			return if let Err(ref err) = res
				&& is_auth_error(err)
			{
				res.wrap_err("hint: check the password")
			} else {
				res
			};
		}
		Ok(conn) => conn,
	};
	conn.simple_query("SELECT 1")
		.await
		.into_diagnostic()
		.wrap_err("checking connection")?;
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;

	#[tokio::test]
	async fn test_create_pool_valid_connection_string() {
		let connection_string = "postgresql://localhost/test";
		let result = create_pool(connection_string, "test").await;
		// May fail if database doesn't exist, but should not be a parsing error
		if let Err(e) = result {
			let error_msg = format!("{:?}", e);
			assert!(
				!error_msg.contains("parsing connection string"),
				"Should not be a parsing error: {}",
				error_msg
			);
		}
	}

	#[tokio::test]
	async fn test_create_pool_with_full_url() {
		let connection_string = "postgresql://user:pass@localhost:5432/testdb";
		let result = create_pool(connection_string, "test").await;
		// May fail if database doesn't exist or auth fails, but should not be a parsing error
		if let Err(e) = result {
			let error_msg = format!("{:?}", e);
			assert!(
				!error_msg.contains("parsing connection string"),
				"Should not be a parsing error: {}",
				error_msg
			);
		}
	}

	#[tokio::test]
	async fn test_create_pool_with_unix_socket_path() {
		// Test connecting via Unix socket path
		let url = "postgresql:///postgres?host=/var/run/postgresql";
		let result = create_pool(url, "test").await;
		// This may fail if PostgreSQL isn't running or isn't accessible via Unix socket
		// but we can at least verify the parsing works
		match result {
			Ok(_) => {
				// Connection succeeded
			}
			Err(e) => {
				let error_msg = format!("{:?}", e);
				// Verify it's not a parsing error but a connection error
				assert!(
					!error_msg.contains("parsing connection string"),
					"Should not be a parsing error: {}",
					error_msg
				);
			}
		}
	}

	#[tokio::test]
	async fn test_create_pool_with_encoded_unix_socket() {
		// Test connecting via percent-encoded Unix socket path in host
		let url = "postgresql://%2Fvar%2Frun%2Fpostgresql/postgres";
		let result = create_pool(url, "test").await;
		// This may fail if PostgreSQL isn't running, but parsing should work
		match result {
			Ok(_) => {
				// Connection succeeded
			}
			Err(e) => {
				let error_msg = format!("{:?}", e);
				// Verify it's not a parsing error
				assert!(
					!error_msg.contains("parsing connection string"),
					"Should not be a parsing error: {}",
					error_msg
				);
			}
		}
	}

	#[tokio::test]
	async fn test_create_pool_with_no_host() {
		// Test connection with no host specified (should try Unix socket or fallback to localhost)
		let url = "postgresql:///postgres";
		let result = create_pool(url, "test").await;
		// This should either succeed or fail with a connection error, not a parsing error
		match result {
			Ok(_) => {
				// Connection succeeded
			}
			Err(e) => {
				let error_msg = format!("{:?}", e);
				// Verify it's not a parsing error
				assert!(
					!error_msg.contains("parsing connection string"),
					"Should not be a parsing error: {}",
					error_msg
				);
			}
		}
	}

	#[tokio::test]
	async fn test_unix_socket_connection_end_to_end() {
		// Test that we can actually connect and query via Unix socket
		let url = "postgresql:///postgres?host=/var/run/postgresql";
		let result = create_pool(url, "test").await;

		match result {
			Ok(pool) => {
				// If connection succeeded, try a simple query
				let conn = pool.get().await;
				if let Ok(conn) = conn {
					let result = conn.simple_query("SELECT 1 as test").await;
					assert!(result.is_ok(), "Query should succeed");
				}
			}
			Err(e) => {
				let error_msg = format!("{:?}", e);
				// If it failed, make sure it's not a parsing or TLS error
				assert!(
					!error_msg.contains("parsing connection string"),
					"Should not be a parsing error: {}",
					error_msg
				);
				assert!(
					!error_msg.contains("TLS handshake"),
					"Should not be a TLS error for Unix socket: {}",
					error_msg
				);
			}
		}
	}
}

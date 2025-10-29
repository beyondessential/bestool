use std::str::FromStr;
use std::time::Duration;

use miette::{IntoDiagnostic, Result};
use mobc::{Connection, Pool};
use mobc_postgres::{tokio_postgres, PgConnectionManager};
use tokio_postgres::Config;

pub type PgPool = Pool<PgConnectionManager<crate::tls::MakeRustlsConnectWrapper>>;
pub type PgConnection = Connection<PgConnectionManager<crate::tls::MakeRustlsConnectWrapper>>;

/// Create a connection pool from a connection string
pub async fn create_pool(connection_string: &str) -> Result<PgPool> {
	let config = Config::from_str(connection_string).into_diagnostic()?;
	let tls_connector = crate::tls::make_tls_connector()?;
	let manager = PgConnectionManager::new(config, tls_connector);

	Ok(Pool::builder()
		.max_open(10)
		.max_idle(5)
		.max_lifetime(Some(Duration::from_secs(3600)))
		.build(manager))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[tokio::test]
	async fn test_create_pool_valid_connection_string() {
		let connection_string = "postgresql://localhost/test";
		let result = create_pool(connection_string).await;
		assert!(result.is_ok());
	}

	#[tokio::test]
	async fn test_create_pool_with_full_url() {
		let connection_string = "postgresql://user:pass@localhost:5432/testdb";
		let result = create_pool(connection_string).await;
		assert!(result.is_ok());
	}

	#[tokio::test]
	async fn test_pool_can_be_cloned() {
		let connection_string = "postgresql://localhost/test";
		let pool = create_pool(connection_string).await.unwrap();
		let pool_clone = pool.clone();

		assert_eq!(
			pool.state().await.max_open,
			pool_clone.state().await.max_open
		);
	}

	#[tokio::test]
	async fn test_pool_configuration() {
		let connection_string = "postgresql://localhost/test";
		let pool = create_pool(connection_string).await.unwrap();
		let state = pool.state().await;

		assert_eq!(state.max_open, 10);
	}
}

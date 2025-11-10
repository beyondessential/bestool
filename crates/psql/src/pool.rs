use std::{str::FromStr, time::Duration};

use miette::{IntoDiagnostic, Result, WrapErr, miette};
use mobc::{Connection, Pool};
use mobc_postgres::{PgConnectionManager, tokio_postgres};
use tokio_postgres::Config;

pub type PgPool = Pool<PgConnectionManager<crate::tls::MakeRustlsConnectWrapper>>;
pub type PgConnection = Connection<PgConnectionManager<crate::tls::MakeRustlsConnectWrapper>>;

/// Create a connection pool from a connection URL
pub async fn create_pool(url: &str) -> Result<PgPool> {
	let config = Config::from_str(url)
		.into_diagnostic()
		.wrap_err("parsing connection string")?;
	let tls_connector = crate::tls::make_tls_connector().wrap_err("setting up TLS")?;
	let manager = PgConnectionManager::new(config, tls_connector);

	let pool = Pool::builder()
		.max_open(10)
		.max_idle(5)
		.max_lifetime(Some(Duration::from_secs(3600)))
		.build(manager);

	check_pool(&pool).await?;

	Ok(pool)
}

/// Check if we can actually establish a connection
async fn check_pool(pool: &PgPool) -> Result<()> {
	let conn = match pool.get().await {
		Err(mobc::Error::Inner(db_err)) => Err(match db_err.as_db_error() {
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
		)?,
		Err(mobc_err) => Err(mobc_err).into_diagnostic()?,
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

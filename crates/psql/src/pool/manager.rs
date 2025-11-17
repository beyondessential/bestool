use miette::Diagnostic;
use mobc::{Manager, async_trait};
use thiserror::Error;
use tokio_postgres::{CancelToken, Client, Config, NoTls, error::DbError};

#[derive(Debug, Clone)]
pub struct PgConnectionManager {
	config: Config,
	tls: bool,
}

impl PgConnectionManager {
	pub fn new(config: Config, tls: bool) -> Self {
		Self { config, tls }
	}

	pub async fn cancel(&self, token: &CancelToken) -> Result<(), PgError> {
		if self.tls {
			let tls_connector = super::tls::make_tls_connector()?;
			token.cancel_query(tls_connector).await?;
		} else {
			token.cancel_query(NoTls).await?;
		}

		Ok(())
	}
}

#[derive(Error, Debug, Diagnostic)]
pub enum PgError {
	#[error("tls: {0}")]
	Tls(#[from] rustls::Error),
	#[error("postgres: {0}")]
	Pg(#[from] tokio_postgres::Error),
}

impl PgError {
	pub fn as_db_error(&self) -> Option<&DbError> {
		match self {
			PgError::Pg(e) => e.as_db_error(),
			_ => None,
		}
	}
}

#[async_trait]
impl Manager for PgConnectionManager {
	type Connection = Client;
	type Error = PgError;

	async fn connect(&self) -> Result<Self::Connection, Self::Error> {
		if self.tls {
			let tls_connector = super::tls::make_tls_connector()?;
			let (client, conn) = self.config.connect(tls_connector).await?;
			mobc::spawn(conn);
			Ok(client)
		} else {
			let (client, conn) = self.config.connect(NoTls).await?;
			mobc::spawn(conn);
			Ok(client)
		}
	}

	async fn check(&self, conn: Self::Connection) -> Result<Self::Connection, Self::Error> {
		conn.simple_query("").await?;
		Ok(conn)
	}
}

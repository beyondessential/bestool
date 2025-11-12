use percent_encoding::{utf8_percent_encode, AsciiSet, CONTROLS};

/// Characters to encode in userinfo (username:password) part of URL
const USERINFO_ENCODE_SET: &AsciiSet = &CONTROLS
	.add(b':')
	.add(b'@')
	.add(b'/')
	.add(b'?')
	.add(b'#')
	.add(b'[')
	.add(b']')
	.add(b'$');

/// Builds a PostgreSQL connection URL with proper encoding
pub struct ConnectionUrlBuilder {
	pub username: String,
	pub password: Option<String>,
	pub host: String,
	pub port: Option<u16>,
	pub database: String,
}

impl ConnectionUrlBuilder {
	/// Builds the PostgreSQL connection URL with proper percent-encoding
	pub fn build(&self) -> String {
		let (host_with_port, args) = if let Some(port) = self.port {
			(format!("{}:{}", self.host, port), "")
		} else if self.host.starts_with("/") {
			// this covers when the hostname is set to the /var/sock
			// ...we use localhost instead to avoid unix shenanigans
			//    and disable ssl so because that often doesn't work
			("localhost".into(), "?sslmode=disable")
		} else {
			(self.host.clone(), "")
		};

		let encoded_username = utf8_percent_encode(&self.username, USERINFO_ENCODE_SET);
		if let Some(password) = &self.password {
			let encoded_password = utf8_percent_encode(password, USERINFO_ENCODE_SET);
			format!(
				"postgresql://{encoded_username}:{encoded_password}@{host_with_port}/{dbname}{args}",
				dbname=self.database
			)
		} else {
			format!(
				"postgresql://{encoded_username}@{host_with_port}/{dbname}{args}",
				dbname = self.database
			)
		}
	}
}

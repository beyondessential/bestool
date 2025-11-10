use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};

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
		let host_with_port = if let Some(port) = self.port {
			format!("{}:{}", self.host, port)
		} else {
			self.host.clone()
		};

		let encoded_username = utf8_percent_encode(&self.username, USERINFO_ENCODE_SET);
		if let Some(password) = &self.password {
			let encoded_password = utf8_percent_encode(password, USERINFO_ENCODE_SET);
			format!(
				"postgresql://{}:{}@{}/{}",
				encoded_username, encoded_password, host_with_port, self.database
			)
		} else {
			format!(
				"postgresql://{}@{}/{}",
				encoded_username, host_with_port, self.database
			)
		}
	}
}

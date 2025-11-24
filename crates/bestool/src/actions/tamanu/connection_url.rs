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
	.add(b'%')
	.add(b'$');

/// Builds a PostgreSQL connection URL with proper encoding
pub struct ConnectionUrlBuilder {
	pub username: String,
	pub password: Option<String>,
	pub host: String,
	pub port: Option<u16>,
	pub database: String,
	pub ssl_mode: Option<String>,
}

impl ConnectionUrlBuilder {
	/// Builds the PostgreSQL connection URL with proper percent-encoding
	///
	/// Unix socket paths (hosts starting with '/') are formatted as query parameters
	/// for better readability: `postgresql:///dbname?host=/var/run/postgresql`
	pub fn build(&self) -> String {
		let is_unix_socket = self.host.starts_with('/');

		let (host_part, mut query_part) = if is_unix_socket {
			// Use query parameter format for Unix sockets
			let port_param = self
				.port
				.map(|p| format!("&port={}", p))
				.unwrap_or_default();
			("".to_string(), format!("?host={}{}", self.host, port_param))
		} else if let Some(port) = self.port {
			(format!("{}:{}", self.host, port), String::new())
		} else if self.host.is_empty() {
			// Empty host means auto-detect
			(String::new(), String::new())
		} else {
			(self.host.clone(), String::new())
		};

		// Add SSL mode if specified
		if let Some(ssl_mode) = &self.ssl_mode {
			let separator = if query_part.is_empty() { "?" } else { "&" };
			query_part.push_str(&format!("{}sslmode={}", separator, ssl_mode));
		}

		let encoded_username = utf8_percent_encode(&self.username, USERINFO_ENCODE_SET);
		if let Some(password) = &self.password {
			let encoded_password = utf8_percent_encode(password, USERINFO_ENCODE_SET);
			format!(
				"postgresql://{encoded_username}:{encoded_password}@{host_part}/{dbname}{query_part}",
				dbname = self.database
			)
		} else {
			format!(
				"postgresql://{encoded_username}@{host_part}/{dbname}{query_part}",
				dbname = self.database
			)
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_build_with_unix_socket_path() {
		let builder = ConnectionUrlBuilder {
			username: "testuser".to_string(),
			password: Some("testpass".to_string()),
			host: "/var/run/postgresql".to_string(),
			port: None,
			database: "testdb".to_string(),
			ssl_mode: None,
		};
		let url = builder.build();
		assert_eq!(
			url,
			"postgresql://testuser:testpass@/testdb?host=/var/run/postgresql"
		);
	}

	#[test]
	fn test_build_with_unix_socket_path_and_port() {
		let builder = ConnectionUrlBuilder {
			username: "testuser".to_string(),
			password: Some("testpass".to_string()),
			host: "/var/run/postgresql".to_string(),
			port: Some(5433),
			database: "testdb".to_string(),
			ssl_mode: None,
		};
		let url = builder.build();
		assert_eq!(
			url,
			"postgresql://testuser:testpass@/testdb?host=/var/run/postgresql&port=5433"
		);
	}

	#[test]
	fn test_build_with_empty_host() {
		let builder = ConnectionUrlBuilder {
			username: "testuser".to_string(),
			password: Some("testpass".to_string()),
			host: "".to_string(),
			port: None,
			database: "testdb".to_string(),
			ssl_mode: None,
		};
		let url = builder.build();
		assert_eq!(url, "postgresql://testuser:testpass@/testdb");
	}

	#[test]
	fn test_build_with_tcp_host_and_port() {
		let builder = ConnectionUrlBuilder {
			username: "testuser".to_string(),
			password: Some("testpass".to_string()),
			host: "localhost".to_string(),
			port: Some(5432),
			database: "testdb".to_string(),
			ssl_mode: None,
		};
		let url = builder.build();
		assert_eq!(url, "postgresql://testuser:testpass@localhost:5432/testdb");
	}

	#[test]
	fn test_build_without_password() {
		let builder = ConnectionUrlBuilder {
			username: "testuser".to_string(),
			password: None,
			host: "localhost".to_string(),
			port: None,
			database: "testdb".to_string(),
			ssl_mode: None,
		};
		let url = builder.build();
		assert_eq!(url, "postgresql://testuser@localhost/testdb");
	}

	#[test]
	fn test_build_with_special_chars_in_password() {
		let builder = ConnectionUrlBuilder {
			username: "testuser".to_string(),
			password: Some("p@ss:word/test".to_string()),
			host: "localhost".to_string(),
			port: None,
			database: "testdb".to_string(),
			ssl_mode: None,
		};
		let url = builder.build();
		assert_eq!(
			url,
			"postgresql://testuser:p%40ss%3Aword%2Ftest@localhost/testdb"
		);
	}

	#[test]
	fn test_build_with_ssl_mode() {
		let builder = ConnectionUrlBuilder {
			username: "testuser".to_string(),
			password: Some("testpass".to_string()),
			host: "localhost".to_string(),
			port: None,
			database: "testdb".to_string(),
			ssl_mode: Some("disable".to_string()),
		};
		let url = builder.build();
		assert_eq!(
			url,
			"postgresql://testuser:testpass@localhost/testdb?sslmode=disable"
		);
	}

	#[test]
	fn test_build_with_ssl_mode_and_unix_socket() {
		let builder = ConnectionUrlBuilder {
			username: "testuser".to_string(),
			password: Some("testpass".to_string()),
			host: "/var/run/postgresql".to_string(),
			port: None,
			database: "testdb".to_string(),
			ssl_mode: Some("require".to_string()),
		};
		let url = builder.build();
		assert_eq!(
			url,
			"postgresql://testuser:testpass@/testdb?host=/var/run/postgresql&sslmode=require"
		);
	}
}

use std::{fmt, str::FromStr, time::Duration};

use aws_config::SdkConfig;
use aws_sdk_sts::Client as STSClient;
use base64ct::{Base64, Encoding};
use miette::{IntoDiagnostic, Result};
use serde::{Deserialize, Serialize};
use tracing::info;

pub const DELEGATED_TOKEN_VERSION: u8 = 1;

/// AWS Delegated Identity Token.
///
/// This is a Base64-encoded JSON structure containing an access key id, secret key, session token,
/// and expiry time.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DelegatedToken {
	pub version: u8,
	pub access_key_id: String,
	pub secret_access_key: String,
	pub region: Option<String>,
	pub session_token: Option<String>,
	pub expiry: Option<String>,
}

impl FromStr for DelegatedToken {
	type Err = std::io::Error;

	fn from_str(s: &str) -> Result<Self, Self::Err> {
		let json = Base64::decode_vec(s)?;
		let token: DelegatedToken = serde_json::from_slice(&json)?;
		if token.version != DELEGATED_TOKEN_VERSION {
			return Err(std::io::Error::other("Invalid token version"));
		}
		Ok(token)
	}
}

impl fmt::Display for DelegatedToken {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		let json = serde_json::to_string(self).map_err(|_| fmt::Error)?;
		let base = Base64::encode_string(json.as_bytes());
		f.write_str(&base)
	}
}

impl DelegatedToken {
	pub async fn new(
		aws: &SdkConfig,
		expiry: Duration,
		policy: &serde_json::Value,
	) -> Result<Self> {
		let client = STSClient::new(aws);
		let token = client
			.get_federation_token()
			.name(crate::APP_NAME)
			.duration_seconds(expiry.as_secs() as i32)
			.policy(policy.to_string())
			.send()
			.await
			.into_diagnostic()?;

		info!(
			"Created temporary federated user: {:?}",
			token.federated_user.as_ref().unwrap()
		);
		info!(
			"Token expires at: {}",
			token.credentials.as_ref().unwrap().expiration
		);

		Ok(Self {
			version: DELEGATED_TOKEN_VERSION,
			access_key_id: token
				.credentials
				.as_ref()
				.unwrap()
				.access_key_id
				.to_string(),
			secret_access_key: token
				.credentials
				.as_ref()
				.unwrap()
				.secret_access_key
				.to_string(),
			session_token: Some(
				token
					.credentials
					.as_ref()
					.unwrap()
					.session_token
					.to_string(),
			),
			region: aws.region().map(|s| s.to_string()),
			expiry: Some(token.credentials.unwrap().expiration.to_string()),
		})
	}
}

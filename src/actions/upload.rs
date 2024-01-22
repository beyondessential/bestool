use std::{
	collections::BTreeMap,
	fmt,
	io::{Read, Write},
	str::FromStr,
};

use aws_sdk_s3::presigning::PresignedRequest;
use base64ct::{Base64Unpadded, Encoding};
use clap::{Parser, Subcommand};
use flate2::{bufread::ZlibDecoder, write::ZlibEncoder, Compression};
use miette::{IntoDiagnostic, Result};
use minicbor::{Decode, Encode};
use tracing::instrument;

use super::Context;

pub mod cancel;
pub mod file;
pub mod preauth;

/// Upload files to S3.
#[derive(Debug, Clone, Parser)]
pub struct UploadArgs {
	/// Upload subcommand
	#[command(subcommand)]
	pub action: UploadAction,
}

#[derive(Debug, Clone, Subcommand)]
pub enum UploadAction {
	Cancel(cancel::CancelArgs),
	File(file::FileArgs),
	Preauth(preauth::PreauthArgs),
}

pub async fn run(ctx: Context<UploadArgs>) -> Result<()> {
	match ctx.args_top.action.clone() {
		UploadAction::Cancel(subargs) => cancel::run(ctx.with_sub(subargs)).await,
		UploadAction::File(subargs) => file::run(ctx.with_sub(subargs)).await,
		UploadAction::Preauth(subargs) => preauth::run(ctx.with_sub(subargs)).await,
	}
}

#[derive(Debug, Clone, Encode, Decode)]
pub struct UploadId {
	#[n(1)]
	pub bucket: String,
	#[n(2)]
	pub key: String,
	#[n(3)]
	pub id: String,
	#[n(4)]
	pub parts: i32,
}

impl FromStr for UploadId {
	type Err = String;

	fn from_str(s: &str) -> Result<Self, Self::Err> {
		let [bucket, key, id, parts] =
			<[&str; 4]>::try_from(s.splitn(4, '|').collect::<Vec<&str>>()).map_err(|parts| {
				format!(
					"not enough |-separated parts: expected 4, got {}",
					parts.len()
				)
			})?;

		if bucket.is_empty() || key.is_empty() || id.is_empty() || parts.is_empty() {
			return Err("Invalid upload ID (empty parts)".to_string());
		}

		Ok(UploadId {
			bucket: bucket.to_string(),
			key: key.to_string(),
			id: id.to_string(),
			parts: i32::from_str(parts).map_err(|err| err.to_string())?,
		})
	}
}

impl fmt::Display for UploadId {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(f, "{}|{}|{}|{}", self.bucket, self.key, self.id, self.parts)
	}
}

#[derive(Debug, Clone, Encode, Decode)]
#[cbor(map)]
pub struct Token {
	#[n(0)]
	pub version: u8,
	#[n(1)]
	pub id: UploadId,
	#[n(2)]
	pub parts: Vec<TokenPart>,
	#[n(3)]
	pub preauther: String,
}

#[derive(Debug, Clone, Encode, Decode)]
#[cbor(map)]
pub struct TokenPart {
	#[n(1)]
	pub number: i32,
	#[n(2)]
	pub method: String,
	#[n(3)]
	pub uri: String,
	#[n(4)]
	pub headers: BTreeMap<String, String>,
}

#[instrument(skip(parts), level = "debug")]
pub fn encode_token(upload_id: &UploadId, parts: &[PresignedRequest]) -> Result<String> {
	let token = Token {
		version: 0,
		id: upload_id.clone(),
		parts: parts
			.iter()
			.enumerate()
			.map(|(number, part)| TokenPart {
				number: number as i32 + 1,
				method: part.method().to_string(),
				uri: part.uri().to_string(),
				headers: part
					.headers()
					.map(|(k, v)| (k.to_string(), v.to_string()))
					.collect(),
			})
			.collect(),
		preauther: crate::APP_NAME.to_string(),
	};

	let encoded = minicbor::to_vec(&token).into_diagnostic()?;
	let mut e = ZlibEncoder::new(Vec::new(), Compression::best());
	e.write_all(&encoded).into_diagnostic()?;
	let compressed = e.finish().into_diagnostic()?;
	Ok(Base64Unpadded::encode_string(&compressed))
}

#[instrument(skip(token), level = "debug")]
pub fn decode_token(token: &str) -> Result<Token> {
	let compressed = Base64Unpadded::decode_vec(&token).into_diagnostic()?;
	let mut d = ZlibDecoder::new(&compressed[..]);
	let mut encoded = Vec::new();
	d.read_to_end(&mut encoded).into_diagnostic()?;
	minicbor::decode(&encoded).into_diagnostic()
}

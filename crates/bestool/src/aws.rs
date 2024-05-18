use std::num::NonZeroU64;

use aws_config::{
	default_provider::credentials::Builder, AppName, BehaviorVersion, ConfigLoader, Region,
	SdkConfig,
};
use aws_credential_types::Credentials;
use clap::Parser;

pub mod s3;
pub mod token;

/// The minimum size of a part in a multipart upload (excluding the last part).
///
/// This is a hard limit imposed by S3. It is not possible to upload a part smaller than this,
/// unless it's the last part. See <https://docs.aws.amazon.com/AmazonS3/latest/userguide/qfacts.html>.
///
/// Also see: <https://stackoverflow.com/questions/19376136/amazon-s3-your-proposed-upload-is-smaller-than-the-minimum-allowed-size>.
///
/// In practice "5 MiB" is not even enough, it must be a little more than that; we use 6 MiB.
// SAFETY: hardcoded
pub const MINIMUM_MULTIPART_PART_SIZE: NonZeroU64 =
	unsafe { NonZeroU64::new_unchecked(6 * 1024 * 1024) };

/// Include this struct as `#[command(flatten)]` in an Args struct so it can host AWS credentials.
#[derive(Debug, Clone, Parser)]
pub struct AwsArgs {
	/// AWS Access Key ID.
	///
	/// This is the AWS Access Key ID to use for authentication. If not specified here, it will be
	/// taken from the environment variable `AWS_ACCESS_KEY_ID`, or from the AWS credentials file
	/// (usually `~/.aws/credentials`), or from ambient credentials (eg EC2 instance profile).
	#[arg(long, value_name = "KEY_ID")]
	pub aws_access_key_id: Option<String>,

	/// AWS Secret Access Key.
	///
	/// This is the AWS Secret Access Key to use for authentication. If not specified here, it will
	/// be taken from the environment variable `AWS_SECRET_ACCESS_KEY`, or from the AWS credentials
	/// file (usually `~/.aws/credentials`), or from ambient credentials (eg EC2 instance profile).
	#[arg(long, value_name = "SECRET_KEY")]
	pub aws_secret_access_key: Option<String>,

	/// AWS Region.
	///
	/// This is the AWS Region to use for authentication and for the bucket. If not specified here,
	/// it will be taken from the environment variable `AWS_REGION`, or from the AWS credentials
	/// file (usually `~/.aws/credentials`), or from ambient credentials (eg EC2 instance profile).
	#[arg(long, value_name = "REGION")]
	pub aws_region: Option<String>,

	/// AWS Session Token.
	///
	/// This is the AWS Session Token to use for authentication using temporary credentials. If not
	/// specified here, it will be taken from the environment variable `AWS_SESSION_TOKEN` if exists.
	#[arg(long, value_name = "SESSION_TOKEN")]
	pub aws_session_token: Option<String>,

	/// AWS Delegated Identity Token.
	///
	/// This is a Base64-encoded JSON structure containing an access key id, secret key, session
	/// token, and expiry time. It can be generated using `delegate` subcommands or other tooling.
	/// It is used as a more convenient way to pass AWS credentials to `bestool` when using
	/// temporary credentials.
	#[arg(long, value_name = "TOKEN")]
	pub aws_delegated: Option<token::DelegatedToken>,
}

impl AwsArgs {
	fn aws_access_key_id(&self) -> Option<::std::borrow::Cow<'_, str>> {
		self.aws_access_key_id
			.as_deref()
			.map(::std::borrow::Cow::Borrowed)
			.or_else(|| {
				self.aws_delegated
					.as_ref()
					.map(|t| ::std::borrow::Cow::Owned(t.access_key_id.clone()))
			})
	}

	fn aws_secret_access_key(&self) -> Option<::std::borrow::Cow<'_, str>> {
		self.aws_secret_access_key
			.as_deref()
			.map(::std::borrow::Cow::Borrowed)
			.or_else(|| {
				self.aws_delegated
					.as_ref()
					.map(|t| ::std::borrow::Cow::Owned(t.secret_access_key.clone()))
			})
	}

	fn aws_region(&self) -> Option<::std::borrow::Cow<'_, str>> {
		self.aws_region
			.as_deref()
			.or_else(|| {
				self.aws_delegated
					.as_ref()
					.and_then(|t: &token::DelegatedToken| t.region.as_deref())
			})
			.map(::std::borrow::Cow::Borrowed)
	}

	fn aws_session_token(&self) -> Option<::std::borrow::Cow<'_, str>> {
		self.aws_session_token
			.as_deref()
			.or_else(|| {
				self.aws_delegated
					.as_ref()
					.and_then(|t| t.session_token.as_deref())
			})
			.map(::std::borrow::Cow::Borrowed)
	}
}

/// Get AWS config from the environment, or credentials files, or ambient, etc.
pub async fn init(args: &AwsArgs) -> SdkConfig {
	let mut config = ConfigLoader::default()
		.behavior_version(BehaviorVersion::v2024_03_28())
		.app_name(AppName::new(crate::APP_NAME).unwrap());

	if let (Some(key_id), Some(secret)) = (args.aws_access_key_id(), args.aws_secret_access_key()) {
		// instead of having only the keys as credentials provider, we set up a full provider chain
		// and add these credentials to it, so that we can still use ambient credentials, regions,
		// sessions, etc.
		let mut chain = Builder::default().with_custom_credential_source(
			"args",
			Credentials::from_keys(key_id, secret, args.aws_session_token().map(Into::into)),
		);
		if let Some(region) = args.aws_region() {
			chain = chain.region(Region::new(region.into_owned()));
		}
		config = config.credentials_provider(chain.build().await);
	} else if let Some(region) = args.aws_region() {
		// if we don't override the credentials provider, set the region directly here
		config = config.region(Region::new(region.into_owned()));
	}

	config.load().await
}

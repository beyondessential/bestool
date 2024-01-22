use std::{borrow::Cow, num::NonZeroU64};

use aws_config::{
	default_provider::credentials::Builder, AppName, BehaviorVersion, ConfigLoader, Region,
	SdkConfig,
};
use aws_credential_types::Credentials;

pub mod upload;

/// The minimum size of a part in a multipart upload (excluding the last part).
///
/// This is a hard limit imposed by S3. It is not possible to upload a part smaller than this,
/// unless it's the last part. See <https://docs.aws.amazon.com/AmazonS3/latest/userguide/qfacts.html>.
///
/// Also see: <https://stackoverflow.com/questions/19376136/amazon-s3-your-proposed-upload-is-smaller-than-the-minimum-allowed-size>.
///
/// In practice "5 MiB" is not even enough, it must be a little more than that.
// SAFETY: hardcoded
pub const MINIMUM_MULTIPART_PART_SIZE: NonZeroU64 =
	unsafe { NonZeroU64::new_unchecked(6 * 1024 * 1024) };

/// Implement this trait on an Args struct to be able to use it as an AWS credential source.
pub trait AwsArgs {
	/// Get the AWS Access Key ID.
	fn aws_access_key_id(&self) -> Option<Cow<'_, str>>;
	fn aws_secret_access_key(&self) -> Option<Cow<'_, str>>;
	fn aws_region(&self) -> Option<Cow<'_, str>>;
}

macro_rules! standard_aws_args {
	($args:ident) => {
		impl crate::aws::AwsArgs for $args {
			fn aws_access_key_id(&self) -> Option<::std::borrow::Cow<'_, str>> {
				self.aws_access_key_id
					.as_ref()
					.map(|s| ::std::borrow::Cow::Borrowed(s.as_str()))
			}

			fn aws_secret_access_key(&self) -> Option<::std::borrow::Cow<'_, str>> {
				self.aws_secret_access_key
					.as_ref()
					.map(|s| ::std::borrow::Cow::Borrowed(s.as_str()))
			}

			fn aws_region(&self) -> Option<::std::borrow::Cow<'_, str>> {
				self.aws_region
					.as_ref()
					.map(|s| ::std::borrow::Cow::Borrowed(s.as_str()))
			}
		}
	};
}
pub(crate) use standard_aws_args;

/// Get AWS config from the environment, or credentials files, or ambient, etc.
pub async fn init(args: &dyn AwsArgs) -> SdkConfig {
	let mut config = ConfigLoader::default()
		.behavior_version(BehaviorVersion::v2023_11_09())
		.app_name(AppName::new(crate::APP_NAME).unwrap());

	if let (Some(key_id), Some(secret)) = (args.aws_access_key_id(), args.aws_secret_access_key()) {
		// instead of having only the keys as credentials provider, we set up a full provider chain
		// and add these credentials to it, so that we can still use ambient credentials, regions,
		// sessions, etc.
		let mut chain = Builder::default()
			.with_custom_credential_source("args", Credentials::from_keys(key_id, secret, None));
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

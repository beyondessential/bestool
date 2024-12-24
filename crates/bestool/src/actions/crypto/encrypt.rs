pub use algae_cli::cli::encrypt::{self, EncryptArgs};
use miette::Result;

use super::CryptoArgs;
use crate::actions::Context;

pub async fn run(ctx: Context<CryptoArgs, EncryptArgs>) -> Result<()> {
	encrypt::run(ctx.args_sub).await
}

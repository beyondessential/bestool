pub use algae_cli::cli::decrypt::{self, DecryptArgs};
use miette::Result;

use super::CryptoArgs;
use crate::actions::Context;

pub async fn run(ctx: Context<CryptoArgs, DecryptArgs>) -> Result<()> {
	decrypt::run(ctx.args_sub).await
}

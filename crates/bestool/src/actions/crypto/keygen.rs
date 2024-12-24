pub use algae_cli::cli::keygen::{self, KeygenArgs};
use miette::Result;

use super::CryptoArgs;
use crate::actions::Context;

pub async fn run(ctx: Context<CryptoArgs, KeygenArgs>) -> Result<()> {
	keygen::run(ctx.args_sub).await
}

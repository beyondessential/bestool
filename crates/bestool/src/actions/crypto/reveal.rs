pub use algae_cli::cli::reveal::{self, RevealArgs};
use miette::Result;

use super::CryptoArgs;
use crate::actions::Context;

pub async fn run(ctx: Context<CryptoArgs, RevealArgs>) -> Result<()> {
	reveal::run(ctx.args_sub).await
}

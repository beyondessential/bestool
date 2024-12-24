pub use algae_cli::cli::protect::{self, ProtectArgs};
use miette::Result;

use super::CryptoArgs;
use crate::actions::Context;

pub async fn run(ctx: Context<CryptoArgs, ProtectArgs>) -> Result<()> {
	protect::run(ctx.args_sub).await
}

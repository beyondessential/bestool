pub use algae_cli::cli::reveal::{self, RevealArgs};
use miette::Result;

use crate::actions::Context;

pub async fn run(args: RevealArgs, _ctx: Context) -> Result<()> {
	reveal::run(args).await
}

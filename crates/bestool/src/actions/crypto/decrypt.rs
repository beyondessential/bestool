pub use algae_cli::cli::decrypt::{self, DecryptArgs};
use miette::Result;

use crate::actions::Context;

pub async fn run(args: DecryptArgs, _ctx: Context) -> Result<()> {
	decrypt::run(args).await
}

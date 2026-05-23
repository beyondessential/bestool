pub use algae_cli::cli::protect::{self, ProtectArgs};
use miette::Result;

use crate::actions::Context;

pub async fn run(args: ProtectArgs, _ctx: Context) -> Result<()> {
	protect::run(args).await
}

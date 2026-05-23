pub use algae_cli::cli::keygen::{self, KeygenArgs};
use miette::Result;

use crate::actions::Context;

pub async fn run(args: KeygenArgs, _ctx: Context) -> Result<()> {
	keygen::run(args).await
}

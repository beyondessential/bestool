pub use algae_cli::cli::encrypt::{self, EncryptArgs};
use miette::Result;

use crate::actions::Context;

pub async fn run(args: EncryptArgs, _ctx: Context) -> Result<()> {
	encrypt::run(args).await
}

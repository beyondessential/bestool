use clap::{CommandFactory, Parser, ValueEnum};
use clap_complete::{Generator, Shell};

use miette::Result;

/// Generate a shell completions script.
///
/// Provides a completions script or configuration for the given shell.
#[derive(Debug, Clone, Parser)]
pub struct CompletionsArgs {
	/// Shell to generate a completions script for.
	#[arg(long, env = "SHELL")]
	pub shell: ShellCompletion,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum ShellCompletion {
	#[value(alias("/usr/bin/bash"), alias("/bin/bash"))]
	Bash,

	#[value(alias("/usr/bin/elvish"))]
	Elvish,

	#[value(alias("/usr/bin/fish"), alias("/bin/fish"))]
	Fish,

	#[value(alias("/usr/bin/nu"))]
	Nu,

	#[value(alias("/usr/bin/pwsh"))]
	Powershell,

	#[value(alias("/usr/bin/zsh"), alias("/bin/zsh"))]
	Zsh,
}

pub async fn run(args: CompletionsArgs) -> Result<()> {
	fn generate(generator: impl Generator) {
		let mut cmd = crate::args::Args::command();
		clap_complete::generate(
			generator,
			&mut cmd,
			env!("CARGO_PKG_NAME"),
			&mut std::io::stdout(),
		);
	}

	match args.shell {
		ShellCompletion::Bash => generate(Shell::Bash),
		ShellCompletion::Elvish => generate(Shell::Elvish),
		ShellCompletion::Fish => generate(Shell::Fish),
		ShellCompletion::Nu => generate(clap_complete_nushell::Nushell),
		ShellCompletion::Powershell => generate(Shell::PowerShell),
		ShellCompletion::Zsh => generate(Shell::Zsh),
	}

	Ok(())
}

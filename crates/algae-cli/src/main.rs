use algae_cli::cli::*;
use clap::Parser;

/// Simple, user-friendly, encryption commands.
///
/// Algae is a simplified profile of the excellent [age](https://age-encryption.org/v1) format.
///
/// It implements five functions for the most common operations, and tries to be as obvious and
/// hard-to-misuse as possible, without being prohibitively hard to use, and while retaining
/// forward-compatibility with age (all algae products can be used with age, but not all age
/// products may be used with algae).
///
/// To start with, generate a keypair with `algae keygen`. This will generate two files:
/// `identity.txt.age`, a passphrase-protected keypair, and `identity.pub`, the public key in plain.
///
/// To encrypt a file, use `algae encrypt -k identity.pub filename`. As this uses the public key, it
/// doesn't require a passphrase. The encrypted file is written to `filename.age`. To decrypt it,
/// use `algae decrypt -k identity.txt.age filename.age`. As this uses the secret key, it will
/// prompt for its passphrase. The decoded file is written back to `filename` (i.e. without the
/// `.age` suffix).
///
/// To obtain a plaintext `identity.txt` (i.e. to remove the passphrase), use
/// `algae reveal identity.txt.age`. To add a new passphrase on a plaintext identity, use
/// `algae protect identity.txt`. These commands are not special to identity files: you can
/// `protect` (encrypt) and `reveal` (decrypt) arbitrary files with a passphrase.
///
/// Every command has a short help (`-h`), which is useful to recall the name of options, and a
/// long help (`--help`), which contains more details and guide-level information.
#[derive(Parser)]
#[clap(
	version,
	max_term_width = 100,
	after_help = "Use --help for a usage guide.",
	after_long_help = ""
)]
enum Command {
	Decrypt(decrypt::DecryptArgs),
	Encrypt(encrypt::EncryptArgs),
	Keygen(keygen::KeygenArgs),
	Protect(protect::ProtectArgs),
	Reveal(reveal::RevealArgs),
}

fn main() -> miette::Result<()> {
	tokio::runtime::Builder::new_multi_thread()
		.enable_all()
		.build()
		.unwrap()
		.block_on(async {
			let command = Command::parse();
			match command {
				Command::Decrypt(args) => decrypt::run(args).await,
				Command::Encrypt(args) => encrypt::run(args).await,
				Command::Keygen(args) => keygen::run(args).await,
				Command::Protect(args) => protect::run(args).await,
				Command::Reveal(args) => reveal::run(args).await,
			}
		})
}

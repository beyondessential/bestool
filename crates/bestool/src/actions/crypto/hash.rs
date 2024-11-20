use std::{
	fs::File,
	iter,
	path::{Path, PathBuf},
};

use blake3::Hasher;
use clap::Parser;
use merkle_hash::{bytes_to_hex, Algorithm, MerkleTree};
use miette::{bail, miette, Context as _, IntoDiagnostic, Result};
use tracing::{debug, instrument};

use super::{Context, CryptoArgs};

/// Checksum files and folders.
///
/// This uses the BLAKE3 algorithm and expects digests to be prefixed by `b3:` to be future-proof.
#[cfg_attr(docsrs, doc("\n\n**Command**: `bestool crypto hash`"))]
#[derive(Debug, Clone, Parser)]
pub struct HashArgs {
	/// Paths to files and/or folders to compute a checksum for.
	///
	/// One path will generate one checksum.
	#[cfg_attr(docsrs, doc("\n\n**Argument**: paths"))]
	#[arg(required = true)]
	pub paths: Vec<PathBuf>,

	/// Digests to check the generated ones against.
	///
	/// Must be provided in the same order as the inputs.
	#[cfg_attr(docsrs, doc("\n\n**Flag**: `--check DIGEST`"))]
	#[arg(long = "check")]
	pub checks: Vec<String>,

	/// Print just the hashes, not the filenames.
	#[cfg_attr(docsrs, doc("\n\n**Flag**: `-n, --no-filenames`"))]
	#[arg(short, long)]
	pub no_filenames: bool,
}

pub async fn run(ctx: Context<CryptoArgs, HashArgs>) -> Result<()> {
	let HashArgs {
		paths,
		checks,
		no_filenames,
	} = ctx.args_sub;

	let mut mismatches = 0;

	for (path, check) in paths
		.into_iter()
		.zip(checks.into_iter().map(Some).chain(iter::repeat(None)))
	{
		debug!(?path, ?check, "process a path");

		let is_file = match path.metadata().into_diagnostic() {
			Ok(metadata) => match metadata.file_type() {
				ft if ft.is_file() => true,
				ft if ft.is_dir() => false,
				_ => {
					eprintln!("{path:?}\tnot a good file to hash");
					continue;
				}
			},
			Err(err) => {
				eprintln!("{path:?}\tcannot open path: {err:?}");
				continue;
			}
		};

		match if is_file {
			hash_file(&path)
		} else {
			hash_folder(&path)
		} {
			Err(err) => {
				let err = err.wrap_err(format!("hashing {path:?}"));
				eprintln!("{path:?}\t{err:?}");
				mismatches += 1;
			}
			Ok(hash) => {
				if !no_filenames {
					print!("{}\t", path.display());
				}
				print!("{hash}");

				if let Some(check) = check {
					let check = check.trim();
					if check == hash {
						println!("\tOK");
					} else {
						println!("\tMISMATCH!");
						mismatches += 1;
					}
				} else {
					println!();
				}
			}
		}
	}

	if mismatches > 0 {
		bail!("one or more mismatches");
	}

	Ok(())
}

#[instrument(level = "debug")]
fn hash_folder(path: &Path) -> Result<String> {
	debug!(?path, "computing checksum for dir");
	let tree = MerkleTree::builder(path.to_str().ok_or_else(|| miette!("bad path: {path:?}"))?)
		.algorithm(Algorithm::Blake3)
		.hash_names(true)
		.build()
		.map_err(|err| miette!("merkletree error: {err}"))?;
	Ok(format!("b3:{}", bytes_to_hex(tree.root.item.hash)))
}

#[instrument(level = "debug")]
fn hash_file(path: &Path) -> Result<String> {
	let file = File::open(path)
		.into_diagnostic()
		.wrap_err("opening file")?;
	let mut hasher = Hasher::new();
	hasher
		.update_reader(file)
		.into_diagnostic()
		.wrap_err("in blake3")?;
	let hash = hasher.finalize();
	Ok(format!("b3:{}", hash.to_hex()))
}

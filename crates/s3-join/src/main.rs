#[cfg(feature = "lambda")]
mod event_handler;

#[cfg(feature = "lambda")]
#[tokio::main]
async fn main() -> Result<(), lambda_runtime::Error> {
	use lambda_runtime::{run, service_fn, tracing};
	tracing::init_default_subscriber();
	run(service_fn(event_handler::function_handler)).await
}

/// S3 Joiner, meant to be deployed in a Lambda.
///
/// Given an S3 prefix containing bestool-chunked files, it checks which files are completely
/// uploaded, then verifies them, and recombines them as whole files into a different S3 prefix.
///
/// It does this without ever holding whole files (nor even whole chunks) in memory, so it can
/// operate from a minimum-resourced AWS Lambda (or locally) on large files.
///
/// AWS ambient (e.g. environment) credentials must be provided that have read access to the inbox
/// and read-write access to the outbox. Delete access to the inbox is needed if `--delete` is on.
#[cfg(not(feature = "lambda"))]
#[derive(clap::Parser)]
#[clap(version)]
struct Args {
	/// The S3 prefix containing the bestool-chunked files to be joined.
	///
	/// Must be a URL like `s3://bucket-name/prefix`.
	#[clap(long)]
	inbox: String,

	/// The S3 prefix where the joined files will be stored.
	///
	/// Must be a URL like `s3://bucket-name/prefix`. Cannot be the same as `--inbox`.
	#[clap(long)]
	outbox: String,

	/// Delete the inbox files after successfully joining them.
	#[clap(long)]
	delete: bool,
}

#[cfg(not(feature = "lambda"))]
#[tokio::main]
async fn main() -> miette::Result<()> {
	use clap::Parser;
	let args = Args::parse();
	todo!()
}

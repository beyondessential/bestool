//! End-to-end test: drive a real kopia through the re-signing proxy against an
//! S3-compatible backend (MinIO in CI), mirroring the spike.
//!
//! Gated behind the `proxy` feature and `#[ignore]`, and skips cleanly unless
//! the environment names a backend and bucket — so a bare `cargo test` is
//! unaffected. Run in CI with:
//!
//! ```text
//! cargo test -p bestool-kopia --features proxy --test proxy_e2e -- --ignored
//! ```
//!
//! Required env: `KOPIA_PROXY_TEST_ENDPOINT` (e.g. `http://127.0.0.1:9000`),
//! `KOPIA_PROXY_TEST_BUCKET`, `KOPIA_PROXY_TEST_ACCESS_KEY`,
//! `KOPIA_PROXY_TEST_SECRET_KEY`. Optional: `KOPIA_PROXY_TEST_REGION`
//! (default `us-east-1`), `KOPIA_BIN` (default `kopia`).
#![cfg(feature = "proxy")]

use std::{path::Path, process::Stdio, sync::Arc};

use bestool_kopia::proxy::{self, Credentials, S3ProxyConfig, StaticCredentialProvider};
use tokio::process::Command;

fn env(key: &str) -> Option<String> {
	std::env::var(key).ok().filter(|s| !s.is_empty())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs a live S3-compatible backend + kopia; run with --ignored in CI"]
async fn full_lifecycle_through_proxy() {
	let (Some(endpoint), Some(bucket), Some(access_key), Some(secret_key)) = (
		env("KOPIA_PROXY_TEST_ENDPOINT"),
		env("KOPIA_PROXY_TEST_BUCKET"),
		env("KOPIA_PROXY_TEST_ACCESS_KEY"),
		env("KOPIA_PROXY_TEST_SECRET_KEY"),
	) else {
		eprintln!("skipping: KOPIA_PROXY_TEST_* env not set");
		return;
	};
	let region = env("KOPIA_PROXY_TEST_REGION").unwrap_or_else(|| "us-east-1".into());
	let kopia = env("KOPIA_BIN").unwrap_or_else(|| "kopia".into());
	let upstream_host = endpoint
		.split_once("://")
		.expect("endpoint needs a scheme")
		.1
		.to_string();

	let proxy = proxy::spawn(
		S3ProxyConfig {
			upstream: endpoint.clone(),
			upstream_host,
			region: region.clone(),
		},
		Arc::new(StaticCredentialProvider(Credentials {
			access_key,
			secret_key,
			session_token: None,
		})),
	)
	.await
	.expect("spawn proxy");

	// Isolated kopia state, and a unique prefix per run.
	let work = std::env::temp_dir().join(format!("kopia-proxy-e2e-{}", std::process::id()));
	let _ = std::fs::remove_dir_all(&work);
	let data = work.join("data");
	let restored = work.join("restored");
	std::fs::create_dir_all(&data).unwrap();
	std::fs::write(data.join("small.txt"), b"hello sigv4 proxy\n").unwrap();
	// >64 KiB of incompressible data forces a multi-chunk streaming PUT.
	std::fs::write(data.join("big.bin"), pseudo_random(256 * 1024)).unwrap();
	let prefix = format!("kopia-proxy-e2e-{}/", std::process::id());

	let endpoint_arg = proxy.endpoint();
	let connect: Vec<String> = [
		"--endpoint",
		&endpoint_arg,
		"--disable-tls",
		"--access-key",
		"dummyaccesskey",
		"--secret-access-key",
		"dummysecretkeydummysecretkey",
		"--region",
		&region,
		"--bucket",
		&bucket,
		"--prefix",
		&prefix,
	]
	.iter()
	.map(|s| s.to_string())
	.collect();

	let run = |args: Vec<String>| {
		let kopia = kopia.clone();
		let work = work.clone();
		async move { run_kopia(&kopia, &work, &args).await }
	};

	let mut create = vec!["repository".into(), "create".into(), "s3".into()];
	create.extend(connect.clone());
	run(create).await;

	run(vec![
		"snapshot".into(),
		"create".into(),
		data.to_string_lossy().into_owned(),
	])
	.await;

	run(vec!["snapshot".into(), "list".into()]).await;

	let snapshot_id = snapshot_id(&kopia, &work).await;
	run(vec![
		"snapshot".into(),
		"restore".into(),
		snapshot_id,
		restored.to_string_lossy().into_owned(),
	])
	.await;

	assert_eq!(
		std::fs::read(data.join("big.bin")).unwrap(),
		std::fs::read(restored.join("big.bin")).unwrap(),
		"restored big.bin must be byte-identical"
	);
	assert_eq!(
		std::fs::read(data.join("small.txt")).unwrap(),
		std::fs::read(restored.join("small.txt")).unwrap(),
	);

	run(vec![
		"maintenance".into(),
		"set".into(),
		"--owner=me".into(),
	])
	.await;
	run(vec!["maintenance".into(), "run".into(), "--full".into()]).await;

	proxy.shutdown().await;
	let _ = std::fs::remove_dir_all(&work);
}

fn kopia_command(kopia: &str, work: &Path) -> Command {
	let mut cmd = Command::new(kopia);
	cmd.env("HOME", work)
		.env("KOPIA_CONFIG_PATH", work.join("repository.config"))
		.env("KOPIA_PASSWORD", "spikepass123")
		.env("KOPIA_CHECK_FOR_UPDATES", "false");
	for key in [
		"AWS_ACCESS_KEY_ID",
		"AWS_SECRET_ACCESS_KEY",
		"AWS_SESSION_TOKEN",
		"AWS_PROFILE",
	] {
		cmd.env_remove(key);
	}
	cmd
}

async fn run_kopia(kopia: &str, work: &Path, args: &[String]) {
	let output = kopia_command(kopia, work)
		.args(args)
		.output()
		.await
		.unwrap_or_else(|e| panic!("spawn kopia {args:?}: {e}"));
	assert!(
		output.status.success(),
		"kopia {args:?} failed ({})\nstderr: {}",
		output.status,
		String::from_utf8_lossy(&output.stderr)
	);
}

async fn snapshot_id(kopia: &str, work: &Path) -> String {
	let output = kopia_command(kopia, work)
		.args(["snapshot", "list", "--json"])
		.stdout(Stdio::piped())
		.output()
		.await
		.expect("snapshot list --json");
	let json = String::from_utf8_lossy(&output.stdout);
	// Avoid a serde_json dep in the test: pull the first "id" field.
	let marker = "\"id\":";
	let start = json.find(marker).expect("snapshot list has an id") + marker.len();
	let rest = json[start..].trim_start().trim_start_matches('"');
	rest.split('"').next().expect("id value").to_string()
}

/// Deterministic incompressible bytes (xorshift), so the pack blob is large
/// enough to be uploaded as a multi-chunk streaming PUT.
fn pseudo_random(len: usize) -> Vec<u8> {
	let mut state: u64 = 0x9e3779b97f4a7c15;
	let mut out = Vec::with_capacity(len);
	while out.len() < len {
		state ^= state << 13;
		state ^= state >> 7;
		state ^= state << 17;
		out.extend_from_slice(&state.to_le_bytes());
	}
	out.truncate(len);
	out
}

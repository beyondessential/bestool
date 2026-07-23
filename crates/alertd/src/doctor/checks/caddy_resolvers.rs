//! Caddy dynamic upstream resolvers healthcheck (Linux).
//!
//! Caddy's `dynamic a` upstreams resolve through the system resolver by
//! default, which on our Linux hosts means systemd-resolved — a path known to
//! wedge under load and hold requests for seconds at a time (multi-second
//! stall storms on busy sites, e.g. Yap and Kosrae Jul 2026). The ops fix
//! points every dynamic upstream block straight at the container DNS
//! (aardvark-dns) with a `resolvers` directive. This check scans /etc/caddy
//! for `dynamic` upstream blocks missing `resolvers` — each one found is a
//! host still on the systemd-resolved path, fixed by redeploying the ops
//! caddy playbook.

use std::fs;
use std::path::{Path, PathBuf};

use super::SweepContext;
use crate::doctor::check::Check;

const NAME: &str = "caddy_resolvers";
const CADDY_DIR: &str = "/etc/caddy";

pub async fn run(_ctx: SweepContext) -> Check {
	if !cfg!(target_os = "linux") {
		return Check::skip(
			NAME,
			"not supported on this platform",
			"systemd-resolved stall only affects Linux caddy hosts",
		);
	}

	run_against(Path::new(CADDY_DIR))
}

fn run_against(dir: &Path) -> Check {
	if !dir.is_dir() {
		return Check::skip(
			NAME,
			"no caddy config",
			format!("{} does not exist", dir.display()),
		);
	}

	let mut files = Vec::new();
	let mut unreadable = Vec::new();
	collect_files(dir, &mut files, &mut unreadable);

	let mut total_blocks = 0;
	let mut misses = Vec::new();
	for file in &files {
		match fs::read_to_string(file) {
			Ok(content) => {
				let (blocks, missing_lines) = blocks_missing_resolvers(&content);
				total_blocks += blocks;
				misses.extend(
					missing_lines
						.into_iter()
						.map(|line| format!("{}:{line}", file.display())),
				);
			}
			Err(_) => unreadable.push(file.clone()),
		}
	}

	if !misses.is_empty() {
		return Check::fail(
			NAME,
			format!(
				"{} of {total_blocks} dynamic upstream(s) missing resolvers",
				misses.len()
			),
			format!(
				"dynamic upstream block(s) without a resolvers directive resolve via \
				 systemd-resolved, which stalls requests under load; redeploy the ops \
				 caddy playbook: {}",
				misses.join(", ")
			),
		)
		.with_detail("missing", misses);
	}

	if !unreadable.is_empty() {
		// No misses found, but we couldn't see everything — a pass here could
		// be a blind spot, not health.
		return Check::broken(
			NAME,
			"caddy config partially unreadable",
			format!(
				"could not read: {}",
				unreadable
					.iter()
					.map(|p| p.display().to_string())
					.collect::<Vec<_>>()
					.join(", ")
			),
		);
	}

	if total_blocks == 0 {
		return Check::pass(NAME, "no dynamic upstreams");
	}

	Check::pass(
		NAME,
		format!("all {total_blocks} dynamic upstream(s) have resolvers"),
	)
}

/// Recursively gather regular files under `dir`; directories we can't list go
/// in `unreadable` so a pass can't silently skip config.
fn collect_files(dir: &Path, files: &mut Vec<PathBuf>, unreadable: &mut Vec<PathBuf>) {
	let entries = match fs::read_dir(dir) {
		Ok(entries) => entries,
		Err(_) => {
			unreadable.push(dir.to_path_buf());
			return;
		}
	};
	for entry in entries.flatten() {
		let path = entry.path();
		if path.is_dir() {
			collect_files(&path, files, unreadable);
		} else if path.is_file() {
			files.push(path);
		}
	}
}

/// Scan Caddyfile-style content for `dynamic` upstream blocks and report
/// (total blocks found, 1-based line numbers of blocks with no `resolvers`
/// directive inside them).
///
/// ponytail: line/brace scanner, not a Caddyfile parser — fine for the
/// machine-templated configs the ops playbook writes; revisit if configs grow
/// hand-written oddities (braces in quoted strings, etc.).
fn blocks_missing_resolvers(content: &str) -> (usize, Vec<usize>) {
	let mut total = 0;
	let mut misses = Vec::new();

	// (start line, depth the block closes at, has resolvers)
	let mut open_block: Option<(usize, i32, bool)> = None;
	let mut depth = 0i32;

	for (idx, raw_line) in content.lines().enumerate() {
		let line = raw_line.split('#').next().unwrap_or("");
		let mut tokens = line.split_whitespace();
		let first = tokens.next();

		if let Some((start, close_depth, has_resolvers)) = open_block.as_mut() {
			if first == Some("resolvers") {
				*has_resolvers = true;
			}
			let (start, close_depth, has_resolvers) = (*start, *close_depth, *has_resolvers);
			depth += brace_delta(line);
			if depth <= close_depth {
				if !has_resolvers {
					misses.push(start);
				}
				open_block = None;
			}
			continue;
		}

		if first == Some("dynamic") {
			total += 1;
			if line.contains('{') {
				open_block = Some((idx + 1, depth, false));
			} else {
				// blockless `dynamic a` — no sub-directives at all, so no
				// resolvers either
				misses.push(idx + 1);
			}
		}

		depth += brace_delta(line);
	}

	// unterminated block at EOF still counts
	if let Some((start, _, false)) = open_block {
		misses.push(start);
	}

	(total, misses)
}

fn brace_delta(line: &str) -> i32 {
	line.chars().fold(0, |acc, c| match c {
		'{' => acc + 1,
		'}' => acc - 1,
		_ => acc,
	})
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn block_with_resolvers_passes() {
		let content = "reverse_proxy {\n\tdynamic a {\n\t\tname api.tamanu.internal\n\t\tresolvers 10.100.0.1\n\t}\n}\n";
		assert_eq!(blocks_missing_resolvers(content), (1, vec![]));
	}

	#[test]
	fn block_without_resolvers_flagged() {
		let content = "reverse_proxy {\n\tdynamic a {\n\t\tname api.msupply.internal\n\t\tport 8000\n\t}\n}\n";
		assert_eq!(blocks_missing_resolvers(content), (1, vec![2]));
	}

	#[test]
	fn mixed_blocks_flag_only_missing() {
		let content =
			"dynamic a {\n\tname one\n\tresolvers 10.100.0.1\n}\ndynamic a {\n\tname two\n}\n";
		assert_eq!(blocks_missing_resolvers(content), (2, vec![5]));
	}

	#[test]
	fn commented_resolvers_does_not_count() {
		let content = "dynamic a {\n\tname one\n\t# resolvers 10.100.0.1\n}\n";
		assert_eq!(blocks_missing_resolvers(content), (1, vec![1]));
	}

	#[test]
	fn blockless_dynamic_flagged() {
		let content = "reverse_proxy {\n\tdynamic a\n}\n";
		assert_eq!(blocks_missing_resolvers(content), (1, vec![2]));
	}

	#[test]
	fn no_dynamic_upstreams() {
		let content = "example.com {\n\treverse_proxy localhost:3000\n}\n";
		assert_eq!(blocks_missing_resolvers(content), (0, vec![]));
	}

	#[test]
	fn unterminated_block_at_eof_flagged() {
		let content = "dynamic a {\n\tname one\n";
		assert_eq!(blocks_missing_resolvers(content), (1, vec![1]));
	}

	#[test]
	fn nested_braces_stay_in_block() {
		let content = "dynamic a {\n\tsub {\n\t\tinner value\n\t}\n\tresolvers 10.100.0.1\n}\n";
		assert_eq!(blocks_missing_resolvers(content), (1, vec![]));
	}
}

//! `bestool canopy tags` — fetch this device's tags from canopy.
//!
//! Calls the canopy `get_self` endpoint over either the tailscale or mTLS
//! auth path (whichever `CanopyClient` picks), then caches the result next
//! to `server-id` so the tags remain available when canopy is unreachable.
//!
//! Cache is treated as a soft fallback, not a source of truth: a successful
//! online fetch always overwrites it; an offline run consults whatever was
//! last cached and warns that the data may be stale.

use std::{collections::BTreeMap, path::Path, sync::Arc};

use bestool_canopy::{CanopyClient, DEFAULT_CANOPY_URL};
use clap::Parser;
use comfy_table::{Row, Table, presets::NOTHING};
use miette::{IntoDiagnostic, Result, WrapErr, bail, miette};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use bestool_tamanu::{config::load_config, server_info::standard_tags_path};

use crate::{actions::{Context, tamanu::TamanuArgs}, args::Args};

/// Fetch this device's tags from canopy.
///
/// Tags are key→value labels stored server-side in canopy, identifying what
/// role / fleet / labels this device carries; the server's own tags are
/// merged over its group's. The fetch is authenticated by the canopy
/// client (tailscale identity, or mTLS with the device key).
///
/// On a successful fetch the result is cached to disk alongside the
/// `server-id` file; on a failed fetch (canopy unreachable, no auth path,
/// HTTP error) the cached copy — if any — is read and printed instead, with
/// a `cached` flag set in the JSON output and a one-line note in the
/// human-readable output.
#[derive(Debug, Clone, Parser)]
#[clap(verbatim_doc_comment)]
pub struct TagsArgs {
	/// Emit the tags as JSON rather than a human-readable table.
	#[arg(long)]
	pub json: bool,

	/// Skip the network fetch and print whatever's in the cache, without
	/// trying canopy first. Useful for fully-offline diagnostic runs.
	#[arg(long)]
	pub offline: bool,
}

/// On-disk shape for the tags cache.
///
/// Wraps the tag map in a struct so we can carry a `fetched_at` timestamp
/// for "how stale is this cache" reporting. Forward-compatible against
/// future fields by being a named-fields struct rather than just the map.
///
/// Caches written before tags became a key→value map (when they were a bare
/// list) fail to parse and are ignored by [`load_cache`], same as any other
/// unparseable cache.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct TagsCache {
	tags: BTreeMap<String, String>,
	#[serde(default)]
	fetched_at: Option<jiff::Timestamp>,
}

pub async fn run(args: TagsArgs, ctx: Context) -> Result<()> {
	// `--root` only exists on the `tamanu` alias path; under `canopy` there's
	// no `TamanuArgs` in the context, so fall back to default discovery.
	let root_arg = ctx.get::<TamanuArgs>().and_then(|t| t.root.clone());
	let (version, root) = bestool_tamanu::find_tamanu(root_arg.as_deref()).await?;
	let config = Arc::new(load_config(&root, None)?);

	let tamanu_version = version.to_string();
	let cache_path = standard_tags_path();

	let (tags, source) = if args.offline {
		match load_cache(&cache_path)? {
			Some(c) => (c.tags, Source::Cache(c.fetched_at)),
			None => bail!(
				"--offline requested but no cached tags at {} — run online once first",
				cache_path.display()
			),
		}
	} else {
		match fetch_online(&tamanu_version).await {
			Ok(tags) => {
				let cache = TagsCache {
					tags: tags.clone(),
					fetched_at: Some(jiff::Timestamp::now()),
				};
				if let Err(err) = save_cache(&cache_path, &cache) {
					warn!(%err, path = %cache_path.display(), "could not save tags cache");
				}
				let _ = config; // suppress unused; kept so we hold the load_config side effects
				(tags, Source::Live)
			}
			Err(err) => {
				warn!(%err, "canopy fetch failed; falling back to cache");
				match load_cache(&cache_path)? {
					Some(c) => (c.tags, Source::Cache(c.fetched_at)),
					None => return Err(err.wrap_err(format!(
						"canopy unreachable and no cached tags at {}",
						cache_path.display()
					))),
				}
			}
		}
	};

	let use_colours = ctx.require::<Args>().logging.color.enabled();
	if args.json {
		render_json(&tags, source)
	} else {
		render_text(&tags, source, use_colours)
	}
}

#[derive(Debug, Clone, Copy)]
enum Source {
	Live,
	Cache(Option<jiff::Timestamp>),
}

async fn fetch_online(tamanu_version: &str) -> Result<BTreeMap<String, String>> {
	let device_key = bestool_tamanu::server_info::fetch_device_key().await;
	let canopy = CanopyClient::new(
		tamanu_version.to_owned(),
		device_key.as_deref(),
		crate::http::client_builder,
	)
	.await?
		.ok_or_else(|| miette!("no canopy auth path: no tailscale, no device key"))?;

	debug!(
		via = if canopy.is_tailscale().await { "tailscale" } else { "mtls" },
		"fetching tags"
	);

	let base: Url = DEFAULT_CANOPY_URL
		.parse()
		.into_diagnostic()
		.wrap_err("parsing canopy base URL")?;

	// `/public/tags` is reachable from tagged-device tailscale callers
	// (the only mount that admits them); `/tags` is the mTLS path on
	// the main canopy host. Returns the merged server+group tag map.
	let response = canopy
		.get(&base, "/public/tags", "/tags")
		.await
		.wrap_err("GET /tags via canopy")?;

	let status = response.status();
	if !status.is_success() {
		let body = response.text().await.unwrap_or_default();
		bail!("canopy /tags returned {status}: {body}");
	}

	response
		.json::<BTreeMap<String, String>>()
		.await
		.into_diagnostic()
		.wrap_err("decoding canopy tags response")
}

fn render_json(tags: &BTreeMap<String, String>, source: Source) -> Result<()> {
	let payload = serde_json::json!({
		"tags": tags,
		"source": match source {
			Source::Live => "live",
			Source::Cache(_) => "cache",
		},
		"cachedAt": match source {
			Source::Cache(Some(t)) => Some(t.to_string()),
			_ => None,
		},
	});
	let stdout = std::io::stdout();
	serde_json::to_writer_pretty(stdout.lock(), &payload).into_diagnostic()?;
	println!();
	Ok(())
}

fn render_text(tags: &BTreeMap<String, String>, source: Source, use_colours: bool) -> Result<()> {
	if tags.is_empty() {
		println!("(no tags assigned)");
	} else {
		let mut table = Table::new();
		table.load_preset(NOTHING);
		table.set_header(Row::from(vec!["Tag", "Value"]));
		for (key, value) in tags {
			table.add_row(Row::from(vec![key.clone(), value.clone()]));
		}
		println!("{table}");
	}

	match source {
		Source::Live => {}
		Source::Cache(Some(t)) => {
			let note = format!("(cached from {t}; canopy unreachable)");
			if use_colours {
				use owo_colors::OwoColorize as _;
				println!("{}", note.dimmed());
			} else {
				println!("{note}");
			}
		}
		Source::Cache(None) => {
			let note = "(cached; canopy unreachable, age unknown)";
			if use_colours {
				use owo_colors::OwoColorize as _;
				println!("{}", note.dimmed());
			} else {
				println!("{note}");
			}
		}
	}
	Ok(())
}

fn load_cache(path: &Path) -> Result<Option<TagsCache>> {
	match std::fs::read(path) {
		Ok(bytes) => match serde_json::from_slice::<TagsCache>(&bytes) {
			Ok(c) => Ok(Some(c)),
			Err(err) => {
				warn!(%err, path = %path.display(), "ignoring unparseable tags cache");
				Ok(None)
			}
		},
		Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
		Err(err) => Err(err)
			.into_diagnostic()
			.wrap_err_with(|| format!("reading tags cache at {}", path.display())),
	}
}

fn save_cache(path: &Path, cache: &TagsCache) -> Result<()> {
	if let Some(parent) = path.parent()
		&& !parent.exists()
	{
		std::fs::create_dir_all(parent)
			.into_diagnostic()
			.wrap_err_with(|| format!("creating tags cache dir {}", parent.display()))?;
	}
	let json = serde_json::to_vec_pretty(cache)
		.into_diagnostic()
		.wrap_err("serialising tags cache")?;
	let tmp = path.with_extension("json.tmp");
	std::fs::write(&tmp, &json)
		.into_diagnostic()
		.wrap_err_with(|| format!("writing tags cache tempfile at {}", tmp.display()))?;
	std::fs::rename(&tmp, path)
		.into_diagnostic()
		.wrap_err_with(|| format!("renaming tags cache into place at {}", path.display()))?;
	Ok(())
}

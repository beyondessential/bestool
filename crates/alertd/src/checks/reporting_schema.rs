//! Whether this server has the reporting schema canopy offers it.
//!
//! A reporting schema is the set of views a server's reports read from. Half of
//! it follows from the Tamanu version's schema and half from the group's own
//! configuration, so it is built centrally, against a replica of the group's
//! data at that version, and offered back per group.
//!
//! What a server has is stamped into the schema itself by the SQL that built it.
//! This check reads that stamp, compares it against the version canopy offers
//! for the version the server runs, and reports the stamp as a top-level status
//! fact so the fleet view can show which schema each server is on. Applying the
//! offered schema is the check's heal action, so it happens only in the daemon
//! and only when the drift has graded as a failure.

use std::sync::Arc;

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use bestool_canopy::CanopyClient;
use miette::{IntoDiagnostic as _, bail};
use node_semver::Version;
use sha2::{Digest as _, Sha256};

use super::{TamanuCx, fmt_db_error};
use crate::{check::Check, heal::HealOutcome};

const NAME: &str = "reporting_schema";

/// The status fact canopy reads to show which schema a server is on.
const VERSION_FACT: &str = "reportingSchemaVersion";

/// The artifact type a reporting schema is published under.
const ARTIFACT_TYPE: &str = "reporting-schema";

/// Reads the version the built schema stamped on itself. The schema is dropped
/// and recreated wholesale by the SQL that builds it, so the stamp goes with it
/// and can never outlive the schema it describes.
const STAMP_SQL: &str = "SELECT obj_description(oid, 'pg_namespace') AS stamp \
	FROM pg_namespace WHERE nspname = 'reporting'";

pub async fn run(ctx: TamanuCx) -> Check {
	let Some(db) = ctx.db().await else {
		return Check::skip(NAME, "no DB connection", "db unavailable");
	};

	let running = match read_stamp(&db).await {
		Ok(stamp) => stamp,
		Err(err) => {
			return Check::broken(
				NAME,
				"could not read the reporting schema",
				format!(
					"reading the schema's version stamp failed: {}",
					fmt_db_error(&err)
				),
			);
		}
	};

	let Some(canopy) = ctx.canopy.as_ref() else {
		// Offline: say what the server has and grade nothing. Whether it is the
		// right one is canopy's to answer, and canopy is not reachable.
		return with_version(
			Check::skip(
				NAME,
				"canopy unreachable",
				"cannot tell whether this schema is the one offered without asking canopy",
			),
			&running,
		);
	};

	let offered = match offered_schema(canopy, &ctx.version).await {
		Ok(offered) => offered,
		Err(err) if canopy_is_out(&err) => {
			return with_version(
				Check::skip(
					NAME,
					"canopy unreachable",
					format!("cannot tell whether this schema is the one offered: {err}"),
				),
				&running,
			);
		}
		Err(err) => {
			return with_version(
				Check::warning(
					NAME,
					"could not ask canopy what is offered",
					format!("fetching the offered reporting schema failed: {err}"),
				),
				&running,
			);
		}
	};

	let Some(offered) = offered else {
		// Canopy offers none for this version. The pair has not been built yet,
		// which is canopy's own finding to raise, not this server's fault.
		return with_version(
			Check::skip(
				NAME,
				"none offered for this version",
				"canopy has no reporting schema built for the version this server runs",
			),
			&running,
		);
	};

	with_version(grade(&running, &offered), &running)
}

/// What the server's `reporting` schema says about itself.
///
/// A schema with no comment on it is not the same as no schema: something built
/// it that was not this pipeline, so the operator is replacing a schema rather
/// than applying a first one, and whatever reports read from it are reading
/// something nobody can name.
#[derive(Debug, PartialEq, Eq)]
enum Stamp {
	NoSchema,
	Unstamped,
	Applied {
		version: Version,
		/// The digest canopy offered for the build that was applied, where the
		/// schema was applied by this check. A schema built for a version says
		/// nothing about which build of it this is, and a group gets a new
		/// build of the version it already runs whenever its reports are fixed.
		build: Option<String>,
	},
}

/// Longest a stamp may be and still be one. The comment is arbitrary text that
/// anyone with COMMENT rights on the schema can set, and it is published to
/// canopy as a status fact, so what is not plausibly a stamp is read as none
/// rather than carried. A version and an SRI digest is 60-odd characters.
const MAX_STAMP_LEN: usize = 128;

/// Read what the server's `reporting` schema stamped on itself.
async fn read_stamp(db: &tokio_postgres::Client) -> Result<Stamp, tokio_postgres::Error> {
	Ok(match db.query_opt(STAMP_SQL, &[]).await? {
		Some(row) => match row.get::<_, Option<String>>("stamp").map(stamp_of) {
			Some(Some((version, build))) => Stamp::Applied { version, build },
			Some(None) | None => Stamp::Unstamped,
		},
		// No `reporting` schema at all. Not an error: a server that has never
		// had one applied is exactly what this check exists to surface.
		None => Stamp::NoSchema,
	})
}

/// The version a schema comment names, and the build it names after it.
///
/// The parsed version is what is kept, not the text: `v2.60.0` and `2.60.0`
/// name the same schema, and a stamp compared as text would fail a server that
/// has exactly the right one. The build is kept as written, since it is
/// canopy's own digest string and is only ever compared with another of those.
///
/// A schema the builder stamped and nothing has applied yet carries the version
/// alone, so the build is optional. Anything after the version that is not a
/// digest makes the whole comment no stamp: it is arbitrary text anyone with
/// COMMENT rights can set and it rides to canopy as a status fact, so what is
/// carried is only ever what this check wrote.
fn stamp_of(comment: String) -> Option<(Version, Option<String>)> {
	let trimmed = comment.trim();
	if trimmed.is_empty() || trimmed.len() > MAX_STAMP_LEN {
		return None;
	}

	let (version, build) = match trimmed.split_once(char::is_whitespace) {
		Some((version, build)) => (version, Some(build.trim())),
		None => (trimmed, None),
	};

	let build = match build.filter(|b| !b.is_empty()) {
		Some(build) if !is_digest(build) => return None,
		build => build.map(str::to_owned),
	};

	Some((Version::parse(version).ok()?, build))
}

/// Whether `text` is shaped like the Subresource Integrity digest canopy
/// offers, e.g. `sha256-LCTbqp…`. Only the shape: what it digests is canopy's
/// to say, and this check only ever compares one of these with another.
fn is_digest(text: &str) -> bool {
	text.split_once('-').is_some_and(|(algorithm, encoded)| {
		matches!(algorithm, "sha256" | "sha384" | "sha512")
			&& !encoded.is_empty()
			&& encoded
				.bytes()
				.all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'='))
	})
}

/// What the stamp on the server says against what canopy offers.
///
/// Separated from the sweep because this is the whole judgement the check
/// makes, and it is worth being able to state it without a database.
fn grade(running: &Stamp, offered: &Offered) -> Check {
	match running {
		Stamp::Applied { version, build } if version == &offered.version => {
			match (build, &offered.digest) {
				(_, None) => Check::pass(NAME, format!("reporting schema {version}")),
				(Some(build), Some(digest)) if build == digest => {
					Check::pass(NAME, format!("reporting schema {version}"))
				}
				_ => Check::fail(
					NAME,
					format!("reporting schema {version}, a newer build offered"),
					"the server's reports read from an earlier build of this version's schema",
				),
			}
		}
		Stamp::Applied { version, .. } => Check::fail(
			NAME,
			format!("reporting schema {version}, offered {}", offered.version),
			"the server's reports read from a schema built for a different version",
		),
		Stamp::Unstamped => Check::fail(
			NAME,
			format!("reporting schema unstamped, offered {}", offered.version),
			"the server has a reporting schema that names no version, so what its \
			 reports read from cannot be told apart from any other build",
		),
		Stamp::NoSchema => Check::fail(
			NAME,
			"no reporting schema",
			"canopy offers one for the version this server runs, and the server has none",
		),
	}
}

/// Carry the stamp as a top-level status fact, so the fleet view can show which
/// schema a server is on without reading into the check's own detail.
fn with_version(check: Check, running: &Stamp) -> Check {
	match running {
		// The version alone: which build of it a server has is this check's to
		// grade, and canopy shows this fact as the schema a server is on.
		Stamp::Applied { version, .. } => {
			check.with_payload_extra(VERSION_FACT, serde_json::Value::from(version.to_string()))
		}
		Stamp::NoSchema | Stamp::Unstamped => check,
	}
}

/// A reporting schema canopy offers, and the version it was built for.
#[derive(Clone)]
struct Offered {
	version: Version,
	id: String,
	download_url: String,
	/// Canopy's digest of the bytes it holds. A rebuild of a pair re-registers
	/// under the same artifact id, so this is what tells two builds of one
	/// version apart.
	digest: Option<String>,
}

/// How long an answer from canopy about what is offered is reused for.
///
/// What canopy offers for one exact Tamanu version changes only when a build
/// publishes a new schema, so asking every sweep is a request per server per
/// minute for the same answer. The window bounds how long the fleet can go on
/// grading against a schema that has just been replaced.
const OFFER_TTL: std::time::Duration = std::time::Duration::from_secs(5 * 60);

/// The last answer canopy gave, and when. Keyed by version so an upgrade asks
/// afresh rather than grading against the schema of the version it left.
static OFFER: std::sync::Mutex<Option<(Version, std::time::Instant, Option<Offered>)>> =
	std::sync::Mutex::new(None);

fn cached_offer(version: &Version) -> Option<Option<Offered>> {
	let held = OFFER.lock().expect("offer cache poisoned");
	held.as_ref().and_then(|(cached, taken, offered)| {
		(cached == version && taken.elapsed() < OFFER_TTL).then(|| offered.clone())
	})
}

fn cache_offer(version: &Version, offered: &Option<Offered>) {
	*OFFER.lock().expect("offer cache poisoned") =
		Some((version.clone(), std::time::Instant::now(), offered.clone()));
}

/// Ask canopy which reporting schema this server is offered.
///
/// The call is authenticated, which is what makes it a group-scoped answer:
/// canopy resolves the caller to its machine and its group and offers that
/// group's schema. The same call unauthenticated would only ever see the
/// artifacts that belong to no group.
async fn offered_schema(
	canopy: &Arc<CanopyClient>,
	version: &Version,
) -> Result<Option<Offered>, bestool_canopy::Error> {
	if let Some(offered) = cached_offer(version) {
		return Ok(offered);
	}

	let artifacts = match canopy.versions_artifacts(&version.to_string()).await {
		Ok(artifacts) => artifacts,
		Err(err) if offers_nothing(&err) => {
			cache_offer(version, &None);
			return Ok(None);
		}
		Err(err) => return Err(err),
	};

	let offered = artifacts.into_iter().find(is_schema).map(|a| Offered {
		version: version.clone(),
		id: a.id.to_string(),
		download_url: a.download_url,
		digest: a.digest,
	});

	cache_offer(version, &offered);
	Ok(offered)
}

/// Whether an artifact is the reporting schema canopy offers this version.
///
/// Canopy resolves a version's artifacts before answering, keeping the most
/// specific of a type, so the schema in the answer is the one to grade against.
fn is_schema(artifact: &bestool_canopy::schema::Artifact) -> bool {
	artifact.artifact_type == ARTIFACT_TYPE
}

/// Whether canopy's answer means it offers nothing for this version, as
/// against the ask itself having failed.
///
/// Canopy answers the artifacts of a version it holds no published, ready
/// release for with a 404, which is the ordinary case for a server on a
/// version canopy has not published. A pair canopy has not built is canopy's
/// own finding to raise rather than this server's fault, so it grades as
/// nothing offered rather than as a warning against the server.
fn offers_nothing(err: &bestool_canopy::Error) -> bool {
	err.status() == Some(bestool_canopy::http::StatusCode::NOT_FOUND)
}

/// Whether the ask failed for canopy's own reasons rather than this server's:
/// the request never landed, or canopy answered with a fault of its own. There
/// is nothing an operator on this server can do about either, and grading them
/// raises the same finding on every server in the fleet for the length of a
/// canopy outage.
fn canopy_is_out(err: &bestool_canopy::Error) -> bool {
	match err {
		// The request never landed.
		bestool_canopy::Error::Transport(_) => true,
		// Anything else that carries no status — an answer that would not
		// decode, above all — is canopy's shape having moved, which has to be
		// visible rather than skipped past.
		other => other
			.status()
			.is_some_and(|status| status.is_server_error()),
	}
}

/// Fetch the bytes of the schema canopy offers.
///
/// Canopy holds a group-scoped artifact itself and serves it only to a caller
/// it is offered to, so the fetch has to carry the device credential the ask
/// carried. An unauthenticated GET of the same URL is answered as a missing
/// artifact, not as a refusal.
async fn fetch_offered(
	canopy: &Arc<CanopyClient>,
	offered: &Offered,
	max_bytes: usize,
) -> Result<String, miette::Report> {
	let mut response = canopy
		.transport()
		.download_artifact(&offered.download_url)
		.await?
		.error_for_status()
		.into_diagnostic()?;

	// A 2xx is not on its own a schema: an HTML page from something between
	// here and canopy would be executed as SQL, and the schema's own SQL drops
	// itself first, so a wrong body destroys what it does not replace.
	let media_type = response
		.headers()
		.get(reqwest::header::CONTENT_TYPE)
		.and_then(|v| v.to_str().ok())
		.map(|v| v.split(';').next().unwrap_or(v).trim().to_ascii_lowercase())
		.unwrap_or_default();
	if !SCHEMA_MEDIA_TYPES.contains(&media_type.as_str()) {
		bail!("the offered reporting schema is {media_type}, not SQL");
	}

	if let Some(len) = response.content_length()
		&& len > max_bytes as u64
	{
		bail!("the offered reporting schema is larger than {max_bytes} bytes");
	}

	let mut sql = Vec::new();
	while let Some(chunk) = response.chunk().await.into_diagnostic()? {
		if sql.len() + chunk.len() > max_bytes {
			bail!("the offered reporting schema is larger than {max_bytes} bytes");
		}
		sql.extend_from_slice(&chunk);
	}
	if sql.is_empty() {
		bail!("the offered reporting schema is empty");
	}

	if let Some(digest) = offered.digest.as_deref()
		&& !matches_digest(&sql, digest)
	{
		bail!("the offered reporting schema is not the bytes canopy named");
	}

	String::from_utf8(sql).into_diagnostic()
}

/// Whether bytes are the ones canopy's digest names.
///
/// Canopy names an artifact's bytes with a sha256 Subresource Integrity digest,
/// and the apply stamps that digest on the schema as the build the server is on,
/// so bytes that hash to anything else are not the schema offered.
fn matches_digest(bytes: &[u8], digest: &str) -> bool {
	digest
		.trim()
		.strip_prefix("sha256-")
		.and_then(|encoded| BASE64.decode(encoded).ok())
		.is_some_and(|named| named == Sha256::digest(bytes).as_slice())
}

/// What a reporting schema may be served as. Canopy hands back whatever media
/// type the registration named, and a schema is SQL text.
const SCHEMA_MEDIA_TYPES: &[&str] = &["application/sql", "text/plain", "application/octet-stream"];

/// Ceiling on a schema, matching what canopy will hold for one.
const MAX_SCHEMA_BYTES: usize = 32 * 1024 * 1024;

/// What a schema artifact may not carry, the apply being one transaction.
const TRANSACTION_CONTROL: [&str; 3] = ["begin", "commit", "rollback"];

/// The transaction control the SQL carries, where it carries any.
///
/// The schema goes to the server as one batch, so a statement that fails partway
/// rolls back the drop the schema opens with. A `BEGIN`, `COMMIT` or `ROLLBACK`
/// of the artifact's own ends that transaction, and a failure after it leaves
/// the server neither the schema it had nor the one offered.
///
/// Only a keyword standing as a statement counts. The same word is ordinary text
/// in an identifier, a literal, a comment or a dollar-quoted body, and a schema
/// refused over one is a server left on whatever it already has.
fn transaction_control(sql: &str) -> Option<&str> {
	let bytes = sql.as_bytes();
	let mut at = 0;
	let mut starts_statement = true;

	while at < bytes.len() {
		let byte = bytes[at];

		if byte.is_ascii_whitespace() {
			at += 1;
			continue;
		}
		if byte == b'-' && bytes.get(at + 1) == Some(&b'-') {
			at = past_line_comment(bytes, at);
			continue;
		}
		if byte == b'/' && bytes.get(at + 1) == Some(&b'*') {
			at = past_block_comment(bytes, at);
			continue;
		}
		if byte == b';' {
			starts_statement = true;
			at += 1;
			continue;
		}

		if is_word_start(byte) {
			let end = word_end(bytes, at);
			let word = &sql[at..end];

			if starts_statement
				&& TRANSACTION_CONTROL
					.iter()
					.any(|keyword| word.eq_ignore_ascii_case(keyword))
			{
				return Some(word);
			}

			// `E'…'`, `B'…'` and the like: the quote belongs to the word before it
			// rather than opening a literal of its own, and only `E` takes
			// backslash escapes.
			at = match bytes.get(end) {
				Some(b'\'') => past_string(bytes, end, word.eq_ignore_ascii_case("e")),
				_ => end,
			};
		} else {
			at = match byte {
				b'\'' => past_string(bytes, at, false),
				b'"' => past_quoted_name(bytes, at),
				b'$' => past_dollar_quote(bytes, at),
				_ => at + 1,
			};
		}

		starts_statement = false;
	}

	None
}

fn is_word_start(byte: u8) -> bool {
	byte.is_ascii_alphabetic() || byte == b'_' || byte >= 0x80
}

fn word_end(bytes: &[u8], at: usize) -> usize {
	let mut end = at;
	while end < bytes.len() && (is_word_start(bytes[end]) || bytes[end].is_ascii_digit()) {
		end += 1;
	}
	end
}

fn past_line_comment(bytes: &[u8], at: usize) -> usize {
	let mut at = at + 2;
	while at < bytes.len() && bytes[at] != b'\n' {
		at += 1;
	}
	at
}

/// Block comments nest, so the first `*/` need not be the one that closes.
fn past_block_comment(bytes: &[u8], at: usize) -> usize {
	let mut at = at + 2;
	let mut depth = 1usize;
	while at < bytes.len() && depth > 0 {
		match (bytes[at], bytes.get(at + 1)) {
			(b'/', Some(b'*')) => {
				depth += 1;
				at += 2;
			}
			(b'*', Some(b'/')) => {
				depth -= 1;
				at += 2;
			}
			_ => at += 1,
		}
	}
	at.min(bytes.len())
}

fn past_string(bytes: &[u8], at: usize, backslash_escapes: bool) -> usize {
	let mut at = at + 1;
	while at < bytes.len() {
		match bytes[at] {
			b'\\' if backslash_escapes => at += 2,
			b'\'' if bytes.get(at + 1) == Some(&b'\'') => at += 2,
			b'\'' => return at + 1,
			_ => at += 1,
		}
	}
	at.min(bytes.len())
}

fn past_quoted_name(bytes: &[u8], at: usize) -> usize {
	let mut at = at + 1;
	while at < bytes.len() {
		match bytes[at] {
			b'"' if bytes.get(at + 1) == Some(&b'"') => at += 2,
			b'"' => return at + 1,
			_ => at += 1,
		}
	}
	at.min(bytes.len())
}

/// Past a dollar-quoted body, or past the `$` where none opens here: a tag may
/// not start with a digit, which is what keeps `$1` a parameter.
fn past_dollar_quote(bytes: &[u8], at: usize) -> usize {
	let tag_end = word_end(bytes, at + 1);
	if bytes.get(tag_end) != Some(&b'$') || bytes.get(at + 1).is_some_and(u8::is_ascii_digit) {
		return at + 1;
	}

	let tag = &bytes[at..=tag_end];
	let mut scan = tag_end + 1;
	while scan + tag.len() <= bytes.len() {
		if &bytes[scan..scan + tag.len()] == tag {
			return scan + tag.len();
		}
		scan += 1;
	}
	bytes.len()
}

/// Apply the schema canopy offers.
///
/// Applying is the one thing on this host that writes to Tamanu's database, so
/// it lives here rather than in the check: heal runs only in the daemon, only
/// when the check graded a failure, and behind the shared backoff.
pub async fn heal(ctx: TamanuCx) -> HealOutcome {
	// A heal that never returns holds the attempt slot for the life of the
	// process, so self-heal stops for this check with nothing to say so.
	match tokio::time::timeout(HEAL_DEADLINE, apply_offered(ctx)).await {
		Ok(outcome) => outcome,
		Err(_) => {
			tracing::warn!("applying the reporting schema did not finish; giving up the attempt");
			HealOutcome::Failed
		}
	}
}

/// Longest one apply may take before the attempt is abandoned.
const HEAL_DEADLINE: std::time::Duration = std::time::Duration::from_secs(10 * 60);

async fn apply_offered(ctx: TamanuCx) -> HealOutcome {
	let (Some(db), Some(canopy)) = (ctx.db().await, ctx.canopy.as_ref()) else {
		return HealOutcome::Deferred;
	};

	let offered = match offered_schema(canopy, &ctx.version).await {
		Ok(Some(offered)) => offered,
		Ok(None) => return HealOutcome::Deferred,
		Err(err) => {
			tracing::warn!("asking canopy for the reporting schema failed: {err}");
			return HealOutcome::Failed;
		}
	};

	// Applying drops the schema before it recreates it, so an artifact that has
	// already been applied and did not leave the stamp it should is not applied
	// again. Retrying it rebuilds the schema on every backoff step, forever,
	// with reports broken through each rebuild.
	if applied_without_stamping(&applied_key(&offered)) {
		tracing::warn!(
			artifact = %offered.id,
			"the offered reporting schema has already been applied without stamping its version"
		);
		return HealOutcome::Deferred;
	}

	let sql = match fetch_offered(canopy, &offered, MAX_SCHEMA_BYTES).await {
		Ok(sql) => sql,
		Err(err) => {
			tracing::warn!("fetching the offered reporting schema failed: {err}");
			return HealOutcome::Failed;
		}
	};

	if let Some(keyword) = transaction_control(&sql) {
		tracing::warn!(
			artifact = %offered.id,
			"the offered reporting schema carries its own {keyword}, so applying it \
			 could leave the server without a reporting schema at all"
		);
		return HealOutcome::Failed;
	}

	let apply = match apply_connection(&ctx.database_url).await {
		Ok(apply) => apply,
		Err(err) => {
			tracing::warn!("opening a connection to apply the reporting schema failed: {err}");
			return HealOutcome::Failed;
		}
	};

	// The schema's own SQL drops and recreates it, so this is not additive and
	// does not need to be made so here. The build goes on in the same batch: a
	// schema recorded as a build it is not would be graded as current and never
	// replaced.
	if let Err(err) = apply.batch_execute(&stamped(&sql, &offered)).await {
		tracing::warn!(version = %offered.version, "applying the reporting schema failed: {err}");
		return HealOutcome::Failed;
	}

	// A heal reported as healed clears the backoff, so an apply that leaves
	// the schema stamped as anything else has to report a failure: otherwise
	// the schema is dropped and rebuilt on every interval, forever.
	match read_stamp(&db).await {
		Ok(Stamp::Applied { version, build })
			if version == offered.version && build == offered.digest =>
		{
			tracing::info!(version = %offered.version, "applied reporting schema");
			HealOutcome::Healed
		}
		Ok(stamp) => {
			tracing::warn!(
				?stamp,
				offered = %offered.version,
				"the applied reporting schema did not stamp the offered version"
			);
			note_unstamped(&applied_key(&offered));
			HealOutcome::Failed
		}
		Err(err) => {
			tracing::warn!(
				"reading back the applied reporting schema's stamp failed: {}",
				fmt_db_error(&err)
			);
			HealOutcome::Failed
		}
	}
}

/// The schema's SQL with the build canopy offered stamped on the end.
///
/// The builder stamps the version, which is what says the SQL came from the
/// pipeline at all. Which build of that version it is, canopy alone knows, so
/// the check records it here and grades against it afterwards.
fn stamped(sql: &str, offered: &Offered) -> String {
	let Some(digest) = offered.digest.as_deref() else {
		return sql.to_owned();
	};

	format!(
		"{sql}\nCOMMENT ON SCHEMA reporting IS '{}';",
		quoted(&format!("{} {digest}", offered.version))
	)
}

/// A string as the body of an SQL literal.
fn quoted(value: &str) -> String {
	value.replace('\'', "''")
}

/// What a build is remembered as, where it applied without stamping.
///
/// The digest and not the artifact id alone: a rebuild of a pair re-registers
/// under the id the artifact already has, and a build that failed to stamp says
/// nothing about the one that replaces it.
fn applied_key(offered: &Offered) -> String {
	match offered.digest.as_deref() {
		Some(digest) => format!("{} {digest}", offered.id),
		None => offered.id.clone(),
	}
}

/// Artifacts this process has applied that did not leave the stamp they should.
fn unstamped() -> &'static std::sync::Mutex<std::collections::HashSet<String>> {
	static IDS: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> =
		std::sync::OnceLock::new();
	IDS.get_or_init(Default::default)
}

fn applied_without_stamping(artifact: &str) -> bool {
	unstamped()
		.lock()
		.expect("unstamped registry poisoned")
		.contains(artifact)
}

fn note_unstamped(artifact: &str) {
	unstamped()
		.lock()
		.expect("unstamped registry poisoned")
		.insert(artifact.to_owned());
}

/// A connection of the apply's own, with ceilings on it.
///
/// The sweep's client is shared by every database-backed check and
/// tokio-postgres serialises what is queued on a connection, so a whole-schema
/// DDL batch on it holds up every other check for as long as the apply runs.
/// Opened through `connect_one` like every other database open in the project,
/// which is what selects TLS for a URL that asks for it. The timeouts bound a
/// batch that cannot get its locks.
async fn apply_connection(database_url: &str) -> Result<tokio_postgres::Client, miette::Report> {
	let client =
		bestool_postgres::pool::connect_one(database_url, "bestool-alertd-reporting-schema")
			.await?;
	client
		.batch_execute("SET statement_timeout = '5min'; SET lock_timeout = '30s'")
		.await
		.into_diagnostic()?;
	Ok(client)
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::{
		check::CheckStatus,
		checks::test_support::{central_ctx, facility_ctx},
	};

	/// The stamp is read from the database, so without one there is nothing to
	/// compare against canopy's offer. It skips rather than failing: a server
	/// whose database this sweep cannot reach is `db_connect`'s finding.
	#[tokio::test]
	async fn no_database_connection_skips() {
		let check = run(facility_ctx()).await;
		assert!(matches!(check.status, CheckStatus::Skip(_)));
		assert!(!check.payload_extras.contains_key(VERSION_FACT));
	}

	/// What [`STAMP_SQL`] reads back, against a real Postgres: no `reporting`
	/// schema at all, one with no comment on it, and a stamped one. The three
	/// readings are what the check branches on, and a query that compiles can
	/// still return the wrong one of them.
	///
	/// Runs inside a transaction that is rolled back, so the database it
	/// borrows keeps whatever it had.
	#[tokio::test]
	async fn the_stamp_query_reads_the_schema_comment() {
		let Some(ctx) = central_ctx().await else {
			return;
		};
		let db = ctx.db().await.expect("central_ctx reached the DB");
		let db = &*db;

		async fn stamp(db: &tokio_postgres::Client) -> Option<Option<String>> {
			db.query_opt(STAMP_SQL, &[])
				.await
				.expect("the stamp query runs")
				.map(|row| row.get::<_, Option<String>>("stamp"))
		}

		db.batch_execute("BEGIN; DROP SCHEMA IF EXISTS reporting CASCADE")
			.await
			.expect("start the transaction");
		let absent = stamp(db).await;

		db.batch_execute("CREATE SCHEMA reporting")
			.await
			.expect("create the schema");
		let unstamped = stamp(db).await;

		db.batch_execute("COMMENT ON SCHEMA reporting IS '2.60.0'")
			.await
			.expect("stamp the schema");
		let stamped = stamp(db).await;

		db.batch_execute("ROLLBACK").await.expect("roll back");

		assert_eq!(absent, None, "no reporting schema is no row");
		assert_eq!(
			unstamped,
			Some(None),
			"a schema with no comment is unstamped"
		);
		assert_eq!(stamped, Some(Some("2.60.0".to_owned())));
	}

	/// The offered SQL drops the schema before recreating it and goes to the
	/// server as one batch, so a statement that fails partway leaves the schema
	/// that was there. An artifact carrying its own `COMMIT` would end that
	/// batch's transaction and lose the guarantee, which is why one carries none.
	// spec: CHK-RSC
	#[tokio::test]
	async fn a_failed_apply_leaves_the_schema_that_was_there() {
		const PROBE: &str = "reporting_apply_probe";

		let Ok(db) = bestool_postgres::pool::connect_one(
			"postgresql://localhost/tamanu-central",
			"bestool-alertd-test",
		)
		.await
		else {
			return;
		};

		db.batch_execute(&format!(
			"DROP SCHEMA IF EXISTS {PROBE} CASCADE; CREATE SCHEMA {PROBE}; \
			 COMMENT ON SCHEMA {PROBE} IS '2.59.0'"
		))
		.await
		.expect("seed the schema the server already has");

		let applied = db
			.batch_execute(&format!(
				"DROP SCHEMA {PROBE} CASCADE; CREATE SCHEMA {PROBE}; \
				 COMMENT ON SCHEMA {PROBE} IS '2.60.0'; SELECT no_such_function()"
			))
			.await;

		let stamp: Option<String> = db
			.query_opt(
				"SELECT obj_description(oid, 'pg_namespace') AS stamp \
				 FROM pg_namespace WHERE nspname = $1",
				&[&PROBE],
			)
			.await
			.expect("read the stamp back")
			.and_then(|row| row.get("stamp"));

		db.batch_execute(&format!("DROP SCHEMA IF EXISTS {PROBE} CASCADE"))
			.await
			.expect("clean up");

		assert!(applied.is_err(), "the batch should have failed");
		assert_eq!(stamp.as_deref(), Some("2.59.0"));
	}

	/// Whether the schema a server has is the offered one is canopy's to
	/// answer, so an unreachable canopy grades nothing rather than grading the
	/// server against a stamp it cannot check.
	#[tokio::test]
	async fn an_unreachable_canopy_grades_nothing() {
		let Some(ctx) = central_ctx().await else {
			return;
		};
		let check = run(ctx).await;

		assert!(
			matches!(check.status, CheckStatus::Skip(_)),
			"got {:?}",
			check.to_wire()["result"]
		);
		assert!(check.summary.contains("canopy"), "{}", check.summary);
	}

	fn http_error(status: u16) -> bestool_canopy::Error {
		bestool_canopy::Error::Http(bestool_canopy::CanopyHttpError {
			status: bestool_canopy::http::StatusCode::from_u16(status).unwrap(),
			path: "/versions/2.60.0/artifacts".to_owned(),
			body: bestool_canopy::bytes::Bytes::new(),
		})
	}

	fn artifact(kind: &str) -> bestool_canopy::schema::Artifact {
		serde_json::from_value(serde_json::json!({
			"artifact_type": kind,
			"download_url": "https://canopy.example/s.sql",
			"id": "00000000-0000-0000-0000-000000000000",
			"platform": "any",
		}))
		.expect("an artifact")
	}

	/// Canopy resolves a version's artifacts before answering, so the schema in
	/// its answer is the one graded against.
	#[test]
	fn the_schema_canopy_offers_is_graded_against() {
		assert!(is_schema(&artifact("reporting-schema")));
	}

	/// Other artifact types share the version listing, and an installer is not
	/// a schema.
	#[test]
	fn another_artifact_type_is_not_a_schema() {
		assert!(!is_schema(&artifact("installer")));
	}

	/// A version canopy has not published has no artifacts of any kind, which
	/// it answers with a 404. That is canopy owing a build, not this server
	/// being wrong, so it must not land as a finding against the server.
	#[test]
	fn a_version_canopy_has_not_published_offers_nothing() {
		assert!(offers_nothing(&http_error(404)));
	}

	/// Anything else is the ask failing, which the server does want to hear
	/// about. Collapsing these into "nothing offered" would hide a canopy that
	/// is refusing or broken behind a silent skip.
	#[test]
	fn a_canopy_that_answered_badly_is_not_an_absent_offer() {
		for status in [401, 403, 500, 502, 503] {
			assert!(
				!offers_nothing(&http_error(status)),
				"{status} is the ask failing"
			);
		}
	}

	fn v(version: &str) -> Version {
		Version::parse(version).expect("a version")
	}

	/// What canopy offers, with no digest: the shape every deployment had before
	/// canopy held the bytes, and the one these gradings are about.
	fn offered(version: &str) -> Offered {
		Offered {
			version: v(version),
			id: "cccccccc-cccc-cccc-cccc-cccccccccccc".to_string(),
			download_url: "https://canopy.example/artifacts/schema.sql".to_string(),
			digest: None,
		}
	}

	fn applied(version: &str) -> Stamp {
		Stamp::Applied {
			version: v(version),
			build: None,
		}
	}

	#[test]
	fn a_matching_stamp_passes() {
		let check = grade(&applied("2.60.0"), &offered("2.60.0"));
		assert!(matches!(check.status, CheckStatus::Pass));
	}

	/// A stamp is compared as a version, not as text: the SQL that writes it is
	/// not this codebase, and `v2.60.0` names the schema `2.60.0` does.
	#[test]
	fn a_stamp_written_differently_still_matches() {
		for written in ["v2.60.0", "2.60.0+build7"] {
			let (version, build) = stamp_of(written.to_owned()).expect("parses as a version");
			assert!(
				matches!(
					grade(&Stamp::Applied { version, build }, &offered("2.60.0")).status,
					CheckStatus::Pass
				),
				"{written} names the offered schema"
			);
		}
	}

	#[test]
	fn a_different_stamp_fails_and_names_both() {
		let check = grade(&applied("2.59.0"), &offered("2.60.0"));
		assert!(matches!(check.status, CheckStatus::Fail(_)));
		// Both versions belong in the summary: which one the server is on is
		// the thing an operator needs, not just that it is wrong.
		assert!(check.summary.contains("2.59.0"), "{}", check.summary);
		assert!(check.summary.contains("2.60.0"), "{}", check.summary);
	}

	#[test]
	fn no_schema_at_all_fails() {
		let check = grade(&Stamp::NoSchema, &offered("2.60.0"));
		assert!(matches!(check.status, CheckStatus::Fail(_)));
	}

	/// A `reporting` schema with no comment on it is a different finding from
	/// having none: something built it that was not this pipeline, and the
	/// summary has to say so or an operator reads "no reporting schema" against
	/// a server whose reports are reading from one.
	#[test]
	fn an_unstamped_schema_is_not_an_absent_one() {
		let unstamped = grade(&Stamp::Unstamped, &offered("2.60.0"));
		let absent = grade(&Stamp::NoSchema, &offered("2.60.0"));

		assert!(matches!(unstamped.status, CheckStatus::Fail(_)));
		assert_ne!(unstamped.summary, absent.summary);
		assert!(
			unstamped.summary.contains("2.60.0"),
			"{}",
			unstamped.summary
		);

		// Neither reports a version: there is none to report, and a stale fact
		// would read as a server sitting on a schema it no longer has.
		assert!(
			!with_version(unstamped, &Stamp::Unstamped)
				.payload_extras
				.contains_key(VERSION_FACT)
		);
	}

	#[test]
	fn the_stamp_is_reported_even_when_it_is_wrong() {
		// A server on the wrong schema is exactly when knowing which one it
		// has matters, so the fact rides along with a failure too.
		let check = with_version(
			grade(&applied("2.59.0"), &offered("2.60.0")),
			&applied("2.59.0"),
		);
		assert_eq!(
			check.payload_extras.get(VERSION_FACT),
			Some(&serde_json::Value::from("2.59.0"))
		);
	}

	/// The comment is arbitrary text anyone with COMMENT rights on the schema
	/// can set, and it rides to canopy as a status fact, so what is not
	/// plausibly a version reads as no stamp rather than being carried.
	#[test]
	fn a_comment_that_is_not_a_version_is_not_a_stamp() {
		assert_eq!(stamp_of("  2.60.0 ".to_owned()), Some((v("2.60.0"), None)));
		assert_eq!(stamp_of("   ".to_owned()), None);
		assert_eq!(stamp_of("x".repeat(MAX_STAMP_LEN + 1)), None);
		assert_eq!(stamp_of("built by hand".to_owned()), None);
		assert_eq!(
			stamp_of("2.60.0\n<script>alert(1)</script>".to_owned()),
			None
		);
	}

	/// The digest rides to canopy as a status fact, so only what is shaped like
	/// one is carried: the comment is arbitrary text anyone with COMMENT rights
	/// on the schema can set.
	#[test]
	fn only_a_digest_shaped_build_is_carried() {
		assert_eq!(
			stamp_of("2.60.0 sha256-LCTbqpIiSOs=".to_owned()),
			Some((v("2.60.0"), Some("sha256-LCTbqpIiSOs=".to_owned())))
		);
		assert_eq!(stamp_of("2.60.0 built-by-hand".to_owned()), None);
		assert_eq!(stamp_of("2.60.0 sha256-".to_owned()), None);
		assert_eq!(
			stamp_of("2.60.0 <script>alert(1)</script>".to_owned()),
			None
		);
	}

	/// An answer that would not decode has no status either, and reading it as
	/// an unreachable canopy stops the grading fleet-wide with nothing to say
	/// the check stopped working.
	#[test]
	fn an_answer_that_would_not_decode_is_not_an_unreachable_canopy() {
		assert!(!canopy_is_out(&bestool_canopy::Error::Decode {
			path: "/versions/2.60.0/artifacts".to_owned(),
			source: serde_json::from_str::<u8>("[]").expect_err("a decode error"),
		}));
		assert!(canopy_is_out(&bestool_canopy::Error::transport(
			std::io::Error::other("no route to host")
		)));
	}

	/// A canopy that never answered, or answered with a fault of its own, is
	/// not a finding against this server: it would raise the same one on every
	/// server in the fleet.
	#[test]
	fn a_canopy_fault_is_not_graded_against_the_server() {
		for status in [500, 502, 503] {
			assert!(canopy_is_out(&http_error(status)), "{status} is canopy's");
		}
		for status in [401, 403, 404] {
			assert!(!canopy_is_out(&http_error(status)), "{status} is an answer");
		}
	}

	/// The apply is one batch, so a statement that fails partway rolls back the
	/// drop the schema opens with. An artifact carrying transaction control of
	/// its own ends that transaction, and a failure after it leaves the server
	/// with neither schema.
	// spec: CHK-RSC
	#[test]
	fn an_artifact_carrying_transaction_control_is_refused() {
		for sql in [
			"BEGIN; CREATE SCHEMA reporting;",
			"commit",
			"\n\tROLLBACK;\n",
			"CREATE SCHEMA reporting;\nCOMMIT;",
			"CREATE SCHEMA reporting;\n-- and then\n  CoMmIt;\n",
			"/* header */ BEGIN;",
			"CREATE SCHEMA reporting; /* and then */ commit;",
			"CREATE VIEW v AS SELECT 'commit' AS label; ROLLBACK;",
		] {
			assert!(transaction_control(sql).is_some(), "{sql:?}");
		}
	}

	/// Refusing an artifact leaves the server on whatever schema it has, so only
	/// a keyword standing as a statement counts. The same word is ordinary text
	/// in an identifier, a literal, a comment or a function body.
	// spec: CHK-RSC
	#[test]
	fn the_word_elsewhere_is_not_transaction_control() {
		for sql in [
			"CREATE VIEW v AS SELECT committed_at FROM t;",
			"CREATE VIEW v AS SELECT commit_date, rollback_id, begins FROM t;",
			"CREATE VIEW v AS SELECT t.commit FROM t;",
			"CREATE VIEW v AS SELECT 'commit; rollback;' AS label;",
			"CREATE VIEW v AS SELECT E'it''s \\'; commit;' AS label;",
			"-- commit\nCREATE SCHEMA reporting;",
			"CREATE SCHEMA reporting; -- begin",
			"/* commit; rollback; */\nCREATE SCHEMA reporting;",
			"/* /* begin; */ commit; */ CREATE SCHEMA reporting;",
			"CREATE TABLE t (\"commit\" int);",
			"CREATE TABLE \"t; commit\" (a int);",
			"CREATE PROCEDURE p() AS $$ BEGIN PERFORM 1; COMMIT; END $$ LANGUAGE plpgsql;",
			"CREATE PROCEDURE p() AS $body$ BEGIN PERFORM 1; ROLLBACK; END $body$ LANGUAGE plpgsql;",
			"CREATE SCHEMA reporting;\nCOMMENT ON SCHEMA reporting IS '2.60.0';\n",
		] {
			assert_eq!(transaction_control(sql), None, "{sql:?}");
		}
	}

	/// Canopy names an artifact's bytes with a sha256 Subresource Integrity
	/// digest, and the apply stamps that digest on the schema as the build the
	/// server is on, so anything else is not the schema offered. The two digests
	/// here are canopy's own, so the encoding is checked and not just the hash.
	#[test]
	fn only_the_bytes_canopy_named_are_applied() {
		const SCHEMA: &[u8] = b"CREATE SCHEMA reporting;\n";
		const DIGEST: &str = "sha256-ujw9dykwmiegt+dMTirjNmjYuieQjjSl2U/Y+f9Mn3A=";
		const EMPTY: &str = "sha256-47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU=";

		assert!(matches_digest(SCHEMA, DIGEST));
		assert!(matches_digest(b"", EMPTY));

		assert!(!matches_digest(SCHEMA, EMPTY));
		assert!(!matches_digest(b"CREATE SCHEMA reporting;", DIGEST));

		// Only a sha256 SRI is one canopy could have offered, and bytes that
		// cannot be checked against what was offered are not applied.
		for digest in [
			"",
			"sha256-",
			"sha256-notbase64!",
			"ujw9dykwmiegt+dMTirjNmjYuieQjjSl2U/Y+f9Mn3A=",
			"sha512-ujw9dykwmiegt+dMTirjNmjYuieQjjSl2U/Y+f9Mn3A=",
		] {
			assert!(!matches_digest(SCHEMA, digest), "{digest:?}");
		}
	}

	#[test]
	fn a_server_with_no_schema_reports_no_version() {
		let check = with_version(
			grade(&Stamp::NoSchema, &offered("2.60.0")),
			&Stamp::NoSchema,
		);
		assert!(!check.payload_extras.contains_key(VERSION_FACT));
	}

	/// What canopy offers when it holds the bytes it names, which is what tells
	/// two builds of one version apart.
	fn built(version: &str, digest: &str) -> Offered {
		Offered {
			digest: Some(digest.to_owned()),
			..offered(version)
		}
	}

	fn applied_build(version: &str, digest: &str) -> Stamp {
		Stamp::Applied {
			version: v(version),
			build: Some(digest.to_owned()),
		}
	}

	/// A group gets a new build of the version it already runs whenever its
	/// reports are fixed, so the build is graded alongside the version and a
	/// server on the offered one is current.
	#[test]
	fn the_offered_build_passes() {
		let check = grade(
			&applied_build("2.60.0", "sha256-LCTbqpIiSOs="),
			&built("2.60.0", "sha256-LCTbqpIiSOs="),
		);
		assert!(
			matches!(check.status, CheckStatus::Pass),
			"{}",
			check.summary
		);
	}

	/// The version a schema was built for says nothing about which build of it a
	/// server has, so a stamp naming another build is a server whose reports
	/// read from that one.
	#[test]
	fn an_earlier_build_of_the_offered_version_fails() {
		let check = grade(
			&applied_build("2.60.0", "sha256-LCTbqpIiSOs="),
			&built(
				"2.60.0",
				"sha256-ujw9dykwmiegt+dMTirjNmjYuieQjjSl2U/Y+f9Mn3A=",
			),
		);
		assert!(matches!(check.status, CheckStatus::Fail(_)));
		assert!(check.summary.contains("2.60.0"), "{}", check.summary);
		assert!(
			check.summary.contains("a newer build offered"),
			"{}",
			check.summary
		);
	}

	/// A stamp carrying the version alone names no build, so a server holding
	/// one cannot be shown to have the build canopy offers.
	#[test]
	fn a_stamp_naming_no_build_fails_against_an_offered_one() {
		let check = grade(&applied("2.60.0"), &built("2.60.0", "sha256-LCTbqpIiSOs="));
		assert!(matches!(check.status, CheckStatus::Fail(_)));
		assert!(
			check.summary.contains("a newer build offered"),
			"{}",
			check.summary
		);
	}

	/// The builder stamps the version, and which build of that version this is
	/// only canopy knows, so the apply records the digest offered and the next
	/// sweep grades against it.
	#[test]
	fn the_applied_build_is_stamped_on_the_schema() {
		let sql = stamped(
			"CREATE SCHEMA reporting;",
			&built("2.60.0", "sha256-LCTbqpIiSOs="),
		);
		assert!(sql.starts_with("CREATE SCHEMA reporting;"), "{sql}");
		assert!(
			sql.contains("COMMENT ON SCHEMA reporting IS '2.60.0 sha256-LCTbqpIiSOs='"),
			"{sql}"
		);
	}

	/// An offer naming no bytes leaves the builder's own stamp standing: a
	/// digest recorded for a build canopy did not name would grade as current
	/// against the next offer that does name one.
	#[test]
	fn an_offer_with_no_digest_stamps_nothing() {
		assert_eq!(
			stamped("CREATE SCHEMA reporting;", &offered("2.60.0")),
			"CREATE SCHEMA reporting;"
		);
	}

	/// The digest is canopy's own string and rides into an SQL literal in a
	/// batch that runs as DDL, so an apostrophe in it must not close the
	/// literal.
	#[test]
	fn an_apostrophe_in_the_stamp_cannot_close_the_literal() {
		assert_eq!(quoted("it's"), "it''s");
		let sql = stamped("CREATE SCHEMA reporting;", &built("2.60.0", "sha256-a'b"));
		assert!(sql.contains("IS '2.60.0 sha256-a''b';"), "{sql}");
	}

	/// An artifact that applied without stamping is not applied again, since
	/// retrying it rebuilds the schema on every backoff step with reports broken
	/// through each rebuild. A rebuild of a pair re-registers under the id the
	/// artifact already has, so the digest is remembered with it and the new
	/// build is applied.
	#[test]
	fn a_rebuild_is_not_held_back_by_the_build_it_replaces() {
		let failed = Offered {
			id: "1dd4b8f6-0f57-4a3f-9a2e-5d1c0b7e6a41".to_owned(),
			..built("2.60.0", "sha256-LCTbqpIiSOs=")
		};
		let rebuilt = Offered {
			digest: Some("sha256-ujw9dykwmiegt+dMTirjNmjYuieQjjSl2U/Y+f9Mn3A=".to_owned()),
			..failed.clone()
		};

		assert!(!applied_without_stamping(&applied_key(&failed)));
		note_unstamped(&applied_key(&failed));

		assert!(applied_without_stamping(&applied_key(&failed)));
		assert!(!applied_without_stamping(&applied_key(&rebuilt)));
	}

	/// A loopback socket nothing is listening on, so the tailnet probe refuses
	/// at once and the client takes the device credential's path.
	fn closed_url() -> String {
		let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a loopback socket");
		let addr = listener.local_addr().expect("the socket's address");
		drop(listener);
		format!("http://{addr}")
	}

	/// A canopy on loopback answering one request per connection, in order, and
	/// recording the request lines it was asked. The answers are built against
	/// the URL it ends up on, which is the origin an offer has to name.
	fn serve(
		answers: impl FnOnce(&str) -> Vec<String>,
	) -> (String, Arc<std::sync::Mutex<Vec<String>>>) {
		use std::io::{Read as _, Write as _};

		let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a loopback socket");
		let base = format!(
			"http://{}",
			listener.local_addr().expect("the canopy address")
		);
		let answers = answers(&base);
		let asked = Arc::new(std::sync::Mutex::new(Vec::new()));

		let recorded = Arc::clone(&asked);
		std::thread::spawn(move || {
			for answer in answers {
				let Ok((mut stream, _)) = listener.accept() else {
					return;
				};

				let mut head = Vec::new();
				let mut buf = [0u8; 1024];
				while !head.windows(4).any(|w| w == b"\r\n\r\n") {
					match stream.read(&mut buf) {
						Ok(0) | Err(_) => return,
						Ok(read) => head.extend_from_slice(&buf[..read]),
					}
				}

				let head = String::from_utf8_lossy(&head).into_owned();
				recorded
					.lock()
					.expect("the record of what canopy was asked")
					.push(head.lines().next().unwrap_or_default().to_owned());

				let _ = stream.write_all(answer.as_bytes());
				let _ = stream.flush();
			}
		});

		(base, asked)
	}

	/// A client reaching the canopy at `base` over the device credential, the
	/// path a server off the tailnet takes: the tailnet probe is aimed at a
	/// closed port.
	async fn canopy_at(base: &str) -> Arc<CanopyClient> {
		let key = bestool_canopy::certificates::generate_key().expect("a device key");
		let transport = bestool_canopy::ReqwestTransport::new(
			base.parse().expect("the canopy URL"),
			closed_url().parse().expect("the tailnet URL"),
			Some(&key.serialize_pem()),
			reqwest::Client::builder,
		)
		.await
		.expect("the transport builds")
		.expect("a device key is an auth path");
		Arc::new(CanopyClient::new(transport))
	}

	/// An answer carrying `body` whole.
	fn answer(media_type: &str, body: &str) -> String {
		format!(
			"HTTP/1.1 200 OK\r\nContent-Type: {media_type}\r\nContent-Length: {}\r\n\
			 Connection: close\r\n\r\n{body}",
			body.len()
		)
	}

	/// The offer a fetch follows to the canopy serving it.
	fn offered_from(base: &str) -> Offered {
		Offered {
			download_url: format!("{base}/versions/2.60.0/artifacts/a/download"),
			..offered("2.60.0")
		}
	}

	/// One fetch against a canopy answering exactly `response`.
	async fn fetch(response: &str, max_bytes: usize) -> Result<String, miette::Report> {
		let response = response.to_owned();
		let (base, _asked) = serve(|_| vec![response]);
		let canopy = canopy_at(&base).await;
		fetch_offered(&canopy, &offered_from(&base), max_bytes).await
	}

	/// A 2xx is not on its own a schema: a page from something between the
	/// server and canopy would be executed as SQL, and the schema's own SQL
	/// drops itself first, so a wrong body destroys what it does not replace.
	#[tokio::test]
	async fn a_body_that_is_not_sql_is_refused() {
		let err = fetch(&answer("text/html", "<html>no</html>"), MAX_SCHEMA_BYTES)
			.await
			.expect_err("a page is not a schema");
		assert!(err.to_string().contains("text/html"), "{err}");
	}

	/// Canopy hands back whatever media type the registration named, and a
	/// schema is SQL text under any of them.
	#[tokio::test]
	async fn the_media_types_a_schema_is_served_as_are_taken() {
		for media_type in SCHEMA_MEDIA_TYPES {
			let sql = fetch(
				&answer(media_type, "CREATE SCHEMA reporting;"),
				MAX_SCHEMA_BYTES,
			)
			.await
			.unwrap_or_else(|err| panic!("{media_type}: {err}"));
			assert_eq!(sql, "CREATE SCHEMA reporting;");
		}

		assert!(
			fetch(
				&answer("TEXT/PLAIN; charset=utf-8", "CREATE SCHEMA reporting;"),
				MAX_SCHEMA_BYTES,
			)
			.await
			.is_ok()
		);
	}

	/// Canopy holds nothing larger than the ceiling, so a body declaring more
	/// than it is refused on the declaration, without a byte of it being read.
	#[tokio::test]
	async fn a_body_declaring_more_than_the_ceiling_is_refused() {
		let err = fetch(
			"HTTP/1.1 200 OK\r\nContent-Type: application/sql\r\nContent-Length: 200\r\n\
			 Connection: close\r\n\r\n",
			64,
		)
		.await
		.expect_err("more than the ceiling is not a schema");
		assert!(err.to_string().contains("larger than"), "{err}");
	}

	/// A body declaring no length at all is held to the ceiling as it is read,
	/// so it cannot stream past one it never declared.
	#[tokio::test]
	async fn a_body_that_streams_past_the_ceiling_is_refused() {
		let chunk = "x".repeat(40);
		let err = fetch(
			&format!(
				"HTTP/1.1 200 OK\r\nContent-Type: application/sql\r\n\
				 Transfer-Encoding: chunked\r\nConnection: close\r\n\r\n\
				 28\r\n{chunk}\r\n28\r\n{chunk}\r\n0\r\n\r\n"
			),
			64,
		)
		.await
		.expect_err("a body streamed past the ceiling is not a schema");
		assert!(err.to_string().contains("larger than"), "{err}");
	}

	/// An empty body is not a schema, and applying it would stamp the offered
	/// build onto whatever schema the server already has.
	#[tokio::test]
	async fn an_empty_body_is_not_a_schema() {
		let err = fetch(&answer("application/sql", ""), MAX_SCHEMA_BYTES)
			.await
			.expect_err("an empty body is not a schema");
		assert!(err.to_string().contains("empty"), "{err}");
	}

	/// The offer cache and the registry of applies that did not stamp are one
	/// per process, so the tests that drive them take a turn each.
	async fn one_at_a_time() -> tokio::sync::MutexGuard<'static, ()> {
		static TURN: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
		TURN.lock().await
	}

	/// Canopy's answer for a version's artifacts, offering a schema it holds.
	fn artifacts_answer(base: &str, id: &str) -> String {
		answer(
			"application/json",
			&serde_json::json!([{
				"artifact_type": ARTIFACT_TYPE,
				"download_url": format!("{base}/artifacts/{id}/download"),
				"digest": "sha256-LCTbqpIiSOs=",
				"id": id,
				"platform": "any",
			}])
			.to_string(),
		)
	}

	/// What canopy offers for one exact version changes only when a build
	/// publishes a new schema, so the answer stands for the window rather than
	/// being asked for once a minute by every server in the fleet.
	#[tokio::test]
	async fn an_answer_already_given_is_not_asked_for_again() {
		const ID: &str = "8c4d1b02-9f3e-4a7c-b6d5-0e2f1a3c4b59";

		let _turn = one_at_a_time().await;
		let (base, asked) = serve(|base| vec![artifacts_answer(base, ID); 2]);
		let canopy = canopy_at(&base).await;

		let first = offered_schema(&canopy, &v("9.60.0"))
			.await
			.expect("canopy answered");
		let again = offered_schema(&canopy, &v("9.60.0"))
			.await
			.expect("the answer already given stands");

		assert!(first.is_some() && again.is_some());
		assert_eq!(asked.lock().expect("what canopy was asked").len(), 1);
	}

	/// The window is keyed by version, so a server that has just upgraded asks
	/// afresh rather than grading against the schema of the version it left.
	#[tokio::test]
	async fn an_upgrade_asks_afresh() {
		const ID: &str = "5a7e3c91-2d48-4f60-8b1a-7c9d0e6f2a34";

		let _turn = one_at_a_time().await;
		let (base, asked) = serve(|base| vec![artifacts_answer(base, ID); 2]);
		let canopy = canopy_at(&base).await;

		offered_schema(&canopy, &v("9.61.0"))
			.await
			.expect("canopy answered");
		let upgraded = offered_schema(&canopy, &v("9.61.1"))
			.await
			.expect("canopy answered again");

		assert_eq!(
			upgraded
				.expect("a schema is offered for the version now running")
				.version,
			v("9.61.1")
		);
		let asked = asked.lock().expect("what canopy was asked");
		assert_eq!(asked.len(), 2);
		assert!(asked[1].contains("/versions/9.61.1/artifacts"), "{asked:?}");
	}

	/// A Subresource Integrity digest of some bytes, as canopy names them.
	fn sri(bytes: &[u8]) -> String {
		format!("sha256-{}", BASE64.encode(Sha256::digest(bytes)))
	}

	async fn admin_connection() -> Option<tokio_postgres::Client> {
		bestool_postgres::pool::connect_one(
			"postgresql://localhost/tamanu-central",
			"bestool-alertd-test",
		)
		.await
		.ok()
	}

	/// A context on a database of the test's own, so an apply that drops and
	/// recreates the `reporting` schema cannot touch the one the borrowed
	/// database holds.
	async fn probe_ctx(database: &str) -> Option<TamanuCx> {
		let borrowed = central_ctx().await?;
		let admin = admin_connection().await?;
		admin
			.batch_execute(&format!(
				"DROP DATABASE IF EXISTS \"{database}\" WITH (FORCE)"
			))
			.await
			.ok()?;
		admin
			.batch_execute(&format!("CREATE DATABASE \"{database}\""))
			.await
			.ok()?;

		let database_url = format!("postgresql://localhost/{database}");
		let pool = bestool_postgres::pool::create_pool_sized(
			&database_url,
			"bestool-alertd-test",
			crate::checks::POOL_SIZE,
			bestool_postgres::pool::Prompt::Never,
		)
		.await
		.ok()?;

		Some(TamanuCx {
			database_url,
			pool: Some(pool),
			..borrowed
		})
	}

	async fn drop_probe(database: &str) {
		admin_connection()
			.await
			.expect("the probe database was created, so it can be dropped")
			.batch_execute(&format!(
				"DROP DATABASE IF EXISTS \"{database}\" WITH (FORCE)"
			))
			.await
			.expect("drop the probe database");
	}

	/// A heal reported as healed clears the backoff, so it is reported only
	/// where the schema left behind carries the build canopy offered.
	#[tokio::test]
	async fn an_apply_leaving_the_offered_stamp_heals() {
		const DATABASE: &str = "bestool-alertd-rsc-healed";
		const SQL: &str = "DROP SCHEMA IF EXISTS reporting CASCADE;\nCREATE SCHEMA reporting;\n";

		let _turn = one_at_a_time().await;
		let Some(ctx) = probe_ctx(DATABASE).await else {
			return;
		};

		let version = v("9.62.0");
		let (base, _asked) = serve(|_| vec![answer("application/sql", SQL)]);
		let offered = Offered {
			version: version.clone(),
			id: "0b6f2d14-8e35-4c79-9a2b-1d4e5f607c83".to_owned(),
			download_url: format!("{base}/artifacts/schema/download"),
			digest: Some(sri(SQL.as_bytes())),
		};
		cache_offer(&version, &Some(offered.clone()));

		let ctx = TamanuCx {
			version,
			canopy: Some(canopy_at(&base).await),
			..ctx
		};
		let outcome = apply_offered(ctx.clone()).await;
		let stamp = read_stamp(&ctx.db().await.expect("the probe database")).await;

		drop(ctx);
		drop_probe(DATABASE).await;

		assert_eq!(outcome, HealOutcome::Healed);
		assert_eq!(
			stamp.expect("the stamp reads back"),
			Stamp::Applied {
				version: v("9.62.0"),
				build: offered.digest,
			}
		);
	}

	/// An apply that leaves the schema stamped as anything else is not a heal:
	/// healed clears the backoff, and the schema would be dropped and rebuilt on
	/// every interval with reports broken through each rebuild. The artifact is
	/// remembered so the next attempt does not apply it again.
	#[tokio::test]
	async fn an_apply_stamping_something_else_fails_and_is_not_applied_again() {
		const DATABASE: &str = "bestool-alertd-rsc-unstamped";
		const SQL: &str = "DROP SCHEMA IF EXISTS reporting CASCADE;\nCREATE SCHEMA reporting;\n\
			 COMMENT ON SCHEMA reporting IS '2.59.0';\n";

		let _turn = one_at_a_time().await;
		let Some(ctx) = probe_ctx(DATABASE).await else {
			return;
		};

		let version = v("9.63.0");
		let (base, _asked) = serve(|_| vec![answer("application/sql", SQL)]);
		let offered = Offered {
			version: version.clone(),
			id: "9e1c7a48-3b52-4d06-8f7e-2a6b5c4d3e10".to_owned(),
			download_url: format!("{base}/artifacts/schema/download"),
			digest: None,
		};
		cache_offer(&version, &Some(offered.clone()));

		let ctx = TamanuCx {
			version,
			canopy: Some(canopy_at(&base).await),
			..ctx
		};
		let outcome = apply_offered(ctx.clone()).await;
		let stamp = read_stamp(&ctx.db().await.expect("the probe database")).await;

		drop(ctx);
		drop_probe(DATABASE).await;

		assert_eq!(outcome, HealOutcome::Failed);
		assert_eq!(
			stamp.expect("the stamp reads back"),
			Stamp::Applied {
				version: v("2.59.0"),
				build: None,
			}
		);
		assert!(applied_without_stamping(&applied_key(&offered)));
	}

	/// The sweep's client is shared by every database-backed check, so the apply
	/// takes one of its own, opened the way every other database open in the
	/// project is, and bounds a batch that cannot get its locks.
	#[tokio::test]
	async fn the_apply_opens_a_bounded_connection_of_its_own() {
		let Ok(apply) = apply_connection("postgresql://localhost/tamanu-central").await else {
			return;
		};

		let setting = async |name: &str| -> String {
			apply
				.query_one(&format!("SHOW {name}"), &[])
				.await
				.expect("the setting reads back")
				.get(0)
		};

		assert_eq!(setting("statement_timeout").await, "5min");
		assert_eq!(setting("lock_timeout").await, "30s");
		assert_eq!(
			setting("application_name").await,
			"bestool-alertd-reporting-schema"
		);
	}
}

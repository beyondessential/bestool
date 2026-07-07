//! PostgreSQL tuning sanity check.
//!
//! Flags a postgres instance that was never tuned (still on the compiled-in
//! defaults) or whose tuning has drifted from what the host's RAM warrants —
//! the shape a server takes after a RAM upgrade that was never followed by a
//! re-tune. The expected values mirror how ops tunes postgres: reserve memory
//! for co-hosted workloads, then size the big memory GUCs as fractions of
//! what's left. We don't reproduce every formula — only the high-signal,
//! hardware-derived ones plus the SSD/WAL planner settings that ship wrong by
//! default.
//!
//! Runs on Linux and Windows alike, with one platform carve-out: Windows
//! postgres uses a different shared-memory implementation and degrades with a
//! large shared_buffers, so ops keeps it deliberately low there regardless of
//! RAM — we don't flag a low shared_buffers on Windows as long as it clears a
//! sane floor. It compares the live server against *this host's* RAM, so it
//! only runs when postgres is local; against a remote database it skips, since
//! the local RAM says nothing about the database host.

use sysinfo::{MemoryRefreshKind, RefreshKind, System};

use super::{CheckContext, query_error_check};
use crate::doctor::check::Check;

const GB: i64 = 1024 * 1024 * 1024;

/// random_page_cost at or above this still looks like the spinning-disk
/// default (4.0); ops sets 1.1 for SSDs.
const WARN_RANDOM_PAGE_COST: f64 = 2.0;

/// max_wal_size at or below this is the stock 1GB default; ops sets 8GB.
const WARN_MAX_WAL_SIZE: i64 = GB;

/// On Windows, postgres performs poorly with a large shared_buffers, so ops
/// keeps it deliberately low regardless of RAM. A low shared_buffers on Windows
/// isn't flagged as long as it clears this floor.
const WIN_MIN_SHARED_BUFFERS: i64 = 512 * 1024 * 1024;

/// The portion of total RAM a tuned postgres reserves for everything else,
/// before sizing its own caches. Mirrors the ops tuning: on boxes with real
/// RAM, hold back a fixed slice for the app/caddy/kopia co-tenants; on small
/// boxes, just split down the middle.
fn pg_ram_budget(total: i64) -> i64 {
	if total >= 8 * GB {
		total - 4 * GB
	} else {
		total / 2
	}
}

/// Whether the database is on this same host, so the local RAM describes it.
/// `None`, empty, the loopback names, and a Unix-socket path (leading `/`) are
/// all local; anything else is a remote hostname.
fn is_local(host: Option<&str>) -> bool {
	match host {
		None => true,
		Some(h) => {
			h.is_empty() || h == "localhost" || h == "127.0.0.1" || h == "::1" || h.starts_with('/')
		}
	}
}

/// Whether `actual` is more than a factor of two away from `expected` in either
/// direction. Two-times is deliberately generous: it tolerates rounding and
/// version-to-version differences in the tuning maths, and only fires on the
/// gross mismatches — never tuned, or tuned for a machine half or twice this
/// one's size.
fn off_by_over_2x(actual: i64, expected: i64) -> bool {
	expected > 0 && (actual > 2 * expected || actual * 2 < expected)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Severity {
	Warn,
	Fail,
}

#[derive(Debug, Clone)]
struct Finding {
	severity: Severity,
	message: String,
}

/// The live settings we assess, all normalised to bytes (or their native
/// numeric unit for the cost/concurrency GUCs).
#[derive(Debug, Clone, Copy)]
struct Settings {
	shared_buffers: i64,
	effective_cache_size: i64,
	maintenance_work_mem: i64,
	work_mem: i64,
	max_connections: i64,
	random_page_cost: f64,
	effective_io_concurrency: i64,
	max_wal_size: i64,
}

fn human_bytes(n: i64) -> String {
	let f = n as f64;
	if f >= GB as f64 {
		format!("{:.1}GB", f / GB as f64)
	} else if f >= (1024 * 1024) as f64 {
		format!("{:.0}MB", f / (1024.0 * 1024.0))
	} else if f >= 1024.0 {
		format!("{:.0}kB", f / 1024.0)
	} else {
		format!("{n}B")
	}
}

/// Compare the live settings against what the host's RAM warrants and return
/// every issue found, most-severe-first. Pure so it can be tested without a
/// database.
fn assess(s: &Settings, total_ram: i64, is_windows: bool) -> Vec<Finding> {
	let mut fails = Vec::new();
	let mut warns = Vec::new();

	let budget = pg_ram_budget(total_ram);
	let expected_shared_buffers = budget / 4;
	let expected_effective_cache = (budget / 4) * 3;
	let expected_maintenance = (budget / 16).min(8 * GB);

	// shared_buffers — the single most important GUC, and the clearest tell of
	// an untuned instance. Over-allocation risks starving the OS cache and the
	// co-hosted workloads (OOM); gross under-allocation means it was never
	// tuned for this box. Either way it's a fail.
	if s.shared_buffers > total_ram / 2 {
		fails.push(Finding {
			severity: Severity::Fail,
			message: format!(
				"shared_buffers {} exceeds half of RAM ({}) — over-allocated",
				human_bytes(s.shared_buffers),
				human_bytes(total_ram),
			),
		});
	} else if off_by_over_2x(s.shared_buffers, expected_shared_buffers) {
		let below = s.shared_buffers < expected_shared_buffers;
		// Windows keeps shared_buffers low by design, so don't flag a low value
		// there as long as it clears the floor; a genuinely tiny value still trips.
		let windows_low_ok = is_windows && below && s.shared_buffers >= WIN_MIN_SHARED_BUFFERS;
		if !windows_low_ok {
			let how = if below { "below" } else { "above" };
			fails.push(Finding {
				severity: Severity::Fail,
				message: format!(
					"shared_buffers {} far {how} expected ~{} for {} RAM — postgres looks untuned",
					human_bytes(s.shared_buffers),
					human_bytes(expected_shared_buffers),
					human_bytes(total_ram),
				),
			});
		}
	}

	// work_mem is per-sort-node, so a busy server can hold many multiples of it
	// at once. Bounded crudely by max_connections, the worst case must not be
	// able to exhaust the memory budget.
	let worst_work_mem = s.work_mem.saturating_mul(s.max_connections);
	if worst_work_mem > budget {
		fails.push(Finding {
			severity: Severity::Fail,
			message: format!(
				"work_mem {} × max_connections {} = {} exceeds the {} memory budget — OOM risk",
				human_bytes(s.work_mem),
				s.max_connections,
				human_bytes(worst_work_mem),
				human_bytes(budget),
			),
		});
	} else if worst_work_mem > budget / 2 {
		warns.push(Finding {
			severity: Severity::Warn,
			message: format!(
				"work_mem {} × max_connections {} = {} is over half the {} memory budget",
				human_bytes(s.work_mem),
				s.max_connections,
				human_bytes(worst_work_mem),
				human_bytes(budget),
			),
		});
	}

	if off_by_over_2x(s.effective_cache_size, expected_effective_cache) {
		warns.push(Finding {
			severity: Severity::Warn,
			message: format!(
				"effective_cache_size {} far from expected ~{} for {} RAM",
				human_bytes(s.effective_cache_size),
				human_bytes(expected_effective_cache),
				human_bytes(total_ram),
			),
		});
	}

	if off_by_over_2x(s.maintenance_work_mem, expected_maintenance) {
		warns.push(Finding {
			severity: Severity::Warn,
			message: format!(
				"maintenance_work_mem {} far from expected ~{}",
				human_bytes(s.maintenance_work_mem),
				human_bytes(expected_maintenance),
			),
		});
	}

	if s.random_page_cost >= WARN_RANDOM_PAGE_COST {
		warns.push(Finding {
			severity: Severity::Warn,
			message: format!(
				"random_page_cost {} still near the spinning-disk default (SSD wants ~1.1)",
				s.random_page_cost,
			),
		});
	}

	// effective_io_concurrency is forced to 0 and unsettable on Windows (no
	// posix_fadvise), so it's only a tuning signal on Unix.
	if !is_windows && s.effective_io_concurrency <= 1 {
		warns.push(Finding {
			severity: Severity::Warn,
			message: format!(
				"effective_io_concurrency {} left at the default (SSD wants ~200)",
				s.effective_io_concurrency,
			),
		});
	}

	if s.max_wal_size <= WARN_MAX_WAL_SIZE {
		warns.push(Finding {
			severity: Severity::Warn,
			message: format!(
				"max_wal_size {} at the stock default — expect frequent checkpoints",
				human_bytes(s.max_wal_size),
			),
		});
	}

	fails.extend(warns);
	fails
}

const SETTINGS_QUERY: &str = "
	SELECT
		pg_size_bytes(current_setting('shared_buffers'))       AS shared_buffers,
		pg_size_bytes(current_setting('effective_cache_size')) AS effective_cache_size,
		pg_size_bytes(current_setting('maintenance_work_mem')) AS maintenance_work_mem,
		pg_size_bytes(current_setting('work_mem'))             AS work_mem,
		current_setting('max_connections')::bigint             AS max_connections,
		current_setting('random_page_cost')::float8            AS random_page_cost,
		current_setting('effective_io_concurrency')::bigint    AS effective_io_concurrency,
		pg_size_bytes(current_setting('max_wal_size'))         AS max_wal_size
";

pub async fn run(ctx: CheckContext) -> Check {
	if !is_local(ctx.config.db.host.as_deref()) {
		return Check::skip(
			"pg_tuning",
			"database is not local",
			"tuning is compared against this host's RAM, which doesn't describe a remote database",
		);
	}

	let Some(client) = ctx.db.as_deref() else {
		return Check::skip(
			"pg_tuning",
			"no DB connection",
			"can't read postgres settings; db_connect reports the outage",
		);
	};

	let row = match client.query_one(SETTINGS_QUERY, &[]).await {
		Ok(row) => row,
		Err(err) => return query_error_check("pg_tuning", &err),
	};

	let settings = Settings {
		shared_buffers: row.try_get("shared_buffers").unwrap_or(0),
		effective_cache_size: row.try_get("effective_cache_size").unwrap_or(0),
		maintenance_work_mem: row.try_get("maintenance_work_mem").unwrap_or(0),
		work_mem: row.try_get("work_mem").unwrap_or(0),
		max_connections: row.try_get("max_connections").unwrap_or(0),
		random_page_cost: row.try_get("random_page_cost").unwrap_or(0.0),
		effective_io_concurrency: row.try_get("effective_io_concurrency").unwrap_or(0),
		max_wal_size: row.try_get("max_wal_size").unwrap_or(0),
	};

	let sys = System::new_with_specifics(
		RefreshKind::nothing().with_memory(MemoryRefreshKind::everything()),
	);
	let total_ram = sys.total_memory() as i64;
	if total_ram <= 0 {
		return Check::skip(
			"pg_tuning",
			"could not read host memory",
			"sysinfo reported no total memory; can't derive expected tuning",
		);
	}

	let findings = assess(&settings, total_ram, cfg!(target_os = "windows"));

	let summary = match findings.len() {
		0 => "appears tuned".to_string(),
		1 => findings[0].message.clone(),
		n => format!("{n} tuning issues"),
	};
	let reason = findings
		.iter()
		.map(|f| f.message.as_str())
		.collect::<Vec<_>>()
		.join("; ");

	let check = if findings.iter().any(|f| f.severity == Severity::Fail) {
		Check::fail("pg_tuning", summary, reason)
	} else if !findings.is_empty() {
		Check::warning("pg_tuning", summary, reason)
	} else {
		Check::pass("pg_tuning", summary)
	};

	let budget = pg_ram_budget(total_ram);
	check
		.with_detail("total_ram_bytes", total_ram)
		.with_detail("pg_ram_budget_bytes", budget)
		.with_detail("shared_buffers_bytes", settings.shared_buffers)
		.with_detail("shared_buffers_expected_bytes", budget / 4)
		.with_detail("effective_cache_size_bytes", settings.effective_cache_size)
		.with_detail("maintenance_work_mem_bytes", settings.maintenance_work_mem)
		.with_detail("work_mem_bytes", settings.work_mem)
		.with_detail("max_connections", settings.max_connections)
		.with_detail("random_page_cost", settings.random_page_cost)
		.with_detail(
			"effective_io_concurrency",
			settings.effective_io_concurrency,
		)
		.with_detail("max_wal_size_bytes", settings.max_wal_size)
}

#[cfg(test)]
mod tests {
	use super::*;

	const MB: i64 = 1024 * 1024;

	/// A 32GB box tuned the way ops tunes it: budget = 28GB, shared_buffers =
	/// 7GB, effective_cache_size = 21GB, etc.
	fn tuned_32gb() -> (Settings, i64) {
		let total = 32 * GB;
		let budget = pg_ram_budget(total);
		(
			Settings {
				shared_buffers: budget / 4,
				effective_cache_size: (budget / 4) * 3,
				maintenance_work_mem: (budget / 16).min(8 * GB),
				work_mem: 16 * MB,
				max_connections: 100,
				random_page_cost: 1.1,
				effective_io_concurrency: 200,
				max_wal_size: 8 * GB,
			},
			total,
		)
	}

	#[test]
	fn tuned_instance_has_no_findings() {
		let (s, total) = tuned_32gb();
		assert!(assess(&s, total, false).is_empty());
	}

	#[test]
	fn stock_defaults_on_a_real_box_fail_as_untuned() {
		// A 32GB server still on the compiled-in defaults: shared_buffers 128MB
		// (expected ~7GB), planner GUCs unset.
		let total = 32 * GB;
		let s = Settings {
			shared_buffers: 128 * MB,
			effective_cache_size: 4 * GB,
			maintenance_work_mem: 64 * MB,
			work_mem: 4 * MB,
			max_connections: 100,
			random_page_cost: 4.0,
			effective_io_concurrency: 1,
			max_wal_size: GB,
		};
		let findings = assess(&s, total, false);
		assert!(findings.iter().any(|f| f.severity == Severity::Fail));
		assert!(
			findings[0].message.contains("shared_buffers"),
			"shared_buffers fail should sort first: {findings:?}"
		);
	}

	#[test]
	fn over_allocated_shared_buffers_fails() {
		let total = 16 * GB;
		let (mut s, _) = tuned_32gb();
		s.shared_buffers = 10 * GB; // > half of 16GB
		let findings = assess(&s, total, false);
		assert!(
			findings
				.iter()
				.any(|f| f.severity == Severity::Fail && f.message.contains("over-allocated"))
		);
	}

	#[test]
	fn work_mem_blowup_fails() {
		let (mut s, total) = tuned_32gb();
		// 512MB × 100 connections = 50GB, well over the 28GB budget.
		s.work_mem = 512 * MB;
		s.max_connections = 100;
		let findings = assess(&s, total, false);
		assert!(
			findings
				.iter()
				.any(|f| f.severity == Severity::Fail && f.message.contains("work_mem"))
		);
	}

	#[test]
	fn ram_doubled_without_retune_flags_shared_buffers() {
		// Box was tuned for 16GB (shared_buffers 3GB) then RAM doubled to 32GB
		// (expected ~7GB) without a re-tune. 3GB is under 7GB/2, so it trips.
		let total = 32 * GB;
		let mut s = tuned_32gb().0;
		s.shared_buffers = 3 * GB;
		let findings = assess(&s, total, false);
		assert!(
			findings
				.iter()
				.any(|f| f.message.contains("shared_buffers"))
		);
	}

	#[test]
	fn ssd_planner_defaults_only_warn() {
		let (mut s, total) = tuned_32gb();
		s.random_page_cost = 4.0;
		s.effective_io_concurrency = 1;
		let findings = assess(&s, total, false);
		assert!(!findings.is_empty());
		assert!(findings.iter().all(|f| f.severity == Severity::Warn));
		assert!(
			findings
				.iter()
				.any(|f| f.message.contains("random_page_cost"))
		);
		assert!(
			findings
				.iter()
				.any(|f| f.message.contains("effective_io_concurrency"))
		);
	}

	#[test]
	fn low_shared_buffers_not_flagged_on_windows() {
		// A 32GB box where ops capped shared_buffers at 512MB (expected ~7GB) on
		// Windows. That's below expected but clears the floor, so it's fine.
		let total = 32 * GB;
		let mut s = tuned_32gb().0;
		s.shared_buffers = 512 * MB;
		let findings = assess(&s, total, true);
		assert!(
			!findings
				.iter()
				.any(|f| f.message.contains("shared_buffers")),
			"{findings:?}"
		);
	}

	#[test]
	fn low_shared_buffers_still_flagged_on_linux() {
		// The same low value on Linux is still an untuned instance.
		let total = 32 * GB;
		let mut s = tuned_32gb().0;
		s.shared_buffers = 512 * MB;
		let findings = assess(&s, total, false);
		assert!(
			findings
				.iter()
				.any(|f| f.severity == Severity::Fail && f.message.contains("shared_buffers"))
		);
	}

	#[test]
	fn tiny_shared_buffers_flagged_even_on_windows() {
		// Below the 512MB floor, so even Windows flags it as untuned.
		let total = 32 * GB;
		let mut s = tuned_32gb().0;
		s.shared_buffers = 128 * MB;
		let findings = assess(&s, total, true);
		assert!(
			findings
				.iter()
				.any(|f| f.severity == Severity::Fail && f.message.contains("shared_buffers"))
		);
	}

	#[test]
	fn over_allocated_shared_buffers_still_fails_on_windows() {
		// The Windows carve-out only excuses low values; over-allocation still fails.
		let total = 16 * GB;
		let mut s = tuned_32gb().0;
		s.shared_buffers = 10 * GB;
		let findings = assess(&s, total, true);
		assert!(
			findings
				.iter()
				.any(|f| f.severity == Severity::Fail && f.message.contains("over-allocated"))
		);
	}

	#[test]
	fn effective_io_concurrency_not_flagged_on_windows() {
		let (mut s, total) = tuned_32gb();
		s.effective_io_concurrency = 0;
		let findings = assess(&s, total, true);
		assert!(
			!findings
				.iter()
				.any(|f| f.message.contains("effective_io_concurrency"))
		);
	}

	#[test]
	fn default_wal_size_warns() {
		let (mut s, total) = tuned_32gb();
		s.max_wal_size = GB;
		let findings = assess(&s, total, false);
		assert!(
			findings
				.iter()
				.any(|f| f.severity == Severity::Warn && f.message.contains("max_wal_size"))
		);
	}

	#[test]
	fn small_box_at_defaults_is_not_flagged() {
		// On a 1GB box the budget is 512MB, so expected shared_buffers is 128MB
		// — exactly the stock default. A small untuned box shouldn't alarm.
		let total = GB;
		let s = Settings {
			shared_buffers: 128 * MB,
			effective_cache_size: 384 * MB,
			maintenance_work_mem: 32 * MB,
			work_mem: 4 * MB,
			max_connections: 100,
			random_page_cost: 1.1,
			effective_io_concurrency: 200,
			max_wal_size: 8 * GB,
		};
		let findings = assess(&s, total, false);
		assert!(
			!findings
				.iter()
				.any(|f| f.message.contains("shared_buffers")),
			"{findings:?}"
		);
	}

	/// Exercises the live `SETTINGS_QUERY` against a real postgres so a typo or
	/// a removed GUC surfaces as a test failure rather than a BROKEN check in
	/// production. Skips when the test database is unreachable.
	#[tokio::test]
	async fn settings_query_runs_against_real_postgres() {
		use crate::doctor::check::CheckStatus;
		use crate::doctor::checks::test_support::central_ctx;

		let Some(ctx) = central_ctx().await else {
			return;
		};
		let check = run(ctx).await;
		assert!(
			!matches!(check.status, CheckStatus::Broken(_) | CheckStatus::Skip(_)),
			"query should run cleanly against local postgres, got {:?}: {}",
			check.status.wire_result(),
			check.status.reason().unwrap_or_default(),
		);
	}

	#[test]
	fn remote_host_is_not_local() {
		assert!(is_local(None));
		assert!(is_local(Some("localhost")));
		assert!(is_local(Some("127.0.0.1")));
		assert!(is_local(Some("/var/run/postgresql")));
		assert!(!is_local(Some("db.internal.example")));
		assert!(!is_local(Some("10.0.0.5")));
	}
}

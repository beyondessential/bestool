//! PostgreSQL tuning value computation.
//!
//! Produces the settings a PostgreSQL server on a Tamanu host should run with,
//! replicating pgtune's values for an OLTP workload on SSD storage, adjusted for
//! a resource budget that reserves headroom for the co-located application,
//! reverse proxy, and backup tooling.
//!
//! The same budget and expected values back the doctor's tuning health check, so
//! a host tuned from here passes that check.

pub mod conf_block;

/// One kibibyte's worth of the KiB unit these functions work in (i.e. 1).
const KIB: u64 = 1;
/// A mebibyte expressed in KiB.
const MIB: u64 = 1024 * KIB;
/// A gibibyte expressed in KiB.
const GIB: u64 = 1024 * MIB;

/// The host operating system, which changes a handful of tuning values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
	Linux,
	Windows,
}

impl Platform {
	/// The platform this build targets.
	pub const fn current() -> Self {
		if cfg!(target_os = "windows") {
			Self::Windows
		} else {
			Self::Linux
		}
	}

	fn is_windows(self) -> bool {
		matches!(self, Self::Windows)
	}
}

/// Raw host resources, before any budgeting.
#[derive(Debug, Clone, Copy)]
pub struct HostResources {
	/// Total physical memory, in kibibytes.
	pub total_ram_kib: u64,
	/// Number of logical CPUs.
	pub cpus: u32,
}

/// The budgeted resources every tuning value is derived from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budget {
	/// Memory, in kibibytes, that PostgreSQL may size its caches against.
	pub ram_kib: u64,
	/// CPUs PostgreSQL may spread parallel work across.
	pub cpus: u32,
}

/// Everything needed to compute a full tuning set.
#[derive(Debug, Clone, Copy)]
pub struct TuneInputs {
	pub platform: Platform,
	pub resources: HostResources,
	/// The server's major version (e.g. 16).
	pub pg_major: u32,
	/// The `max_connections` to size around.
	pub max_connections: u32,
	/// Whether the running server reports lz4 as a supported WAL compression
	/// method. When unknown, pass `false`.
	pub lz4_wal_supported: bool,
	/// The `temp_file_limit`, in kibibytes, when it could be derived from the
	/// data volume. Emitted only when present.
	pub temp_file_limit_kib: Option<u64>,
}

/// A single `key = value` tuning directive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Setting {
	pub key: &'static str,
	pub value: String,
}

impl Setting {
	fn new(key: &'static str, value: impl Into<String>) -> Self {
		Self {
			key,
			value: value.into(),
		}
	}
}

/// The RAM PostgreSQL is budgeted, in kibibytes.
///
/// Windows hosts are treated as entitled to at most 4 GiB, because they
/// co-locate more workloads and PostgreSQL for Windows does not benefit from a
/// large shared memory area. Elsewhere the budget holds back a fixed 4 GiB on
/// hosts with real RAM, and splits small hosts down the middle.
fn budget_ram_kib(platform: Platform, total_ram_kib: u64) -> u64 {
	if platform.is_windows() {
		(total_ram_kib / 2).min(4 * GIB)
	} else if total_ram_kib >= 8 * GIB {
		total_ram_kib - 4 * GIB
	} else {
		total_ram_kib / 2
	}
}

/// The CPUs PostgreSQL is budgeted, reserving cores for co-located workloads.
fn budget_cpus(cpus: u32) -> u32 {
	if cpus >= 8 {
		cpus - 4
	} else if cpus >= 5 {
		4
	} else if cpus >= 2 {
		2
	} else {
		1
	}
}

/// Compute the resource budget for a host.
pub fn budget(platform: Platform, resources: HostResources) -> Budget {
	Budget {
		ram_kib: budget_ram_kib(platform, resources.total_ram_kib),
		cpus: budget_cpus(resources.cpus),
	}
}

/// Expected `shared_buffers`, in kibibytes, for a budget.
pub fn expected_shared_buffers_kib(budget: &Budget, platform: Platform, pg_major: u32) -> u64 {
	let value = budget.ram_kib / 4;
	if platform.is_windows() && pg_major < 10 {
		value.min(512 * MIB)
	} else {
		value
	}
}

/// Expected `effective_cache_size`, in kibibytes, for a budget.
pub fn expected_effective_cache_kib(budget: &Budget) -> u64 {
	(budget.ram_kib * 3) / 4
}

/// Expected `maintenance_work_mem`, in kibibytes, for a budget.
pub fn expected_maintenance_kib(budget: &Budget, platform: Platform, pg_major: u32) -> u64 {
	let value = budget.ram_kib / 16;
	let limit = if platform.is_windows() && pg_major <= 17 {
		2 * GIB
	} else {
		8 * GIB
	};
	if value >= limit {
		if platform.is_windows() && pg_major <= 17 {
			limit - MIB
		} else {
			limit
		}
	} else {
		value
	}
}

fn expected_work_mem_kib(
	budget: &Budget,
	platform: Platform,
	pg_major: u32,
	max_connections: u32,
) -> u64 {
	let shared = expected_shared_buffers_kib(budget, platform, pg_major);
	let parallel = if budget.cpus >= 4 { budget.cpus } else { 8 };
	let divisor = u64::from(max_connections + parallel) * 3;
	let mut value = (budget.ram_kib.saturating_sub(shared)) / divisor.max(1);
	value = value.max(4 * MIB);
	if platform.is_windows() && pg_major <= 17 {
		value = value.min(2 * GIB - MIB);
	}
	value
}

fn expected_wal_buffers_kib(shared_buffers_kib: u64) -> u64 {
	let raw = (shared_buffers_kib * 3) / 100;
	let capped = raw.clamp(32, 16 * MIB);
	// pgtune snaps a value within the top band up to the full 16 MiB.
	if capped > 14 * MIB && capped < 16 * MIB {
		16 * MIB
	} else {
		capped
	}
}

/// Render a kibibyte quantity with the largest unit that divides it exactly,
/// matching the `kB`/`MB`/`GB` forms PostgreSQL accepts.
fn render_kib(kib: u64) -> String {
	if kib.is_multiple_of(GIB) {
		format!("{}GB", kib / GIB)
	} else if kib.is_multiple_of(MIB) {
		format!("{}MB", kib / MIB)
	} else {
		format!("{kib}kB")
	}
}

/// Compute the full tuning set, in canonical order.
pub fn compute(input: &TuneInputs) -> Vec<Setting> {
	let TuneInputs {
		platform,
		resources,
		pg_major,
		max_connections,
		lz4_wal_supported,
		temp_file_limit_kib,
	} = *input;
	let budget = budget(platform, resources);
	let mut out = Vec::new();

	let shared_buffers = expected_shared_buffers_kib(&budget, platform, pg_major);
	out.push(Setting::new("max_connections", max_connections.to_string()));
	out.push(Setting::new("shared_buffers", render_kib(shared_buffers)));
	out.push(Setting::new(
		"effective_cache_size",
		render_kib(expected_effective_cache_kib(&budget)),
	));
	out.push(Setting::new(
		"maintenance_work_mem",
		render_kib(expected_maintenance_kib(&budget, platform, pg_major)),
	));
	out.push(Setting::new("checkpoint_completion_target", "0.9"));
	out.push(Setting::new(
		"wal_buffers",
		render_kib(expected_wal_buffers_kib(shared_buffers)),
	));
	out.push(Setting::new("default_statistics_target", "100"));
	out.push(Setting::new("random_page_cost", "1.1"));

	// effective_io_concurrency relies on posix_fadvise, absent on Windows.
	if !platform.is_windows() {
		out.push(Setting::new("effective_io_concurrency", "200"));
	}

	out.push(Setting::new(
		"work_mem",
		render_kib(expected_work_mem_kib(
			&budget,
			platform,
			pg_major,
			max_connections,
		)),
	));
	out.push(Setting::new(
		"huge_pages",
		if shared_buffers >= 2 * GIB {
			"try"
		} else {
			"off"
		},
	));
	out.push(Setting::new("min_wal_size", "2GB"));
	out.push(Setting::new("max_wal_size", "8GB"));

	if budget.cpus >= 4 {
		let per_gather = budget.cpus.div_ceil(2).min(4);
		out.push(Setting::new(
			"max_worker_processes",
			budget.cpus.to_string(),
		));
		out.push(Setting::new(
			"max_parallel_workers_per_gather",
			per_gather.to_string(),
		));
		if pg_major >= 10 {
			out.push(Setting::new(
				"max_parallel_workers",
				budget.cpus.to_string(),
			));
		}
		if pg_major >= 11 {
			out.push(Setting::new(
				"max_parallel_maintenance_workers",
				per_gather.to_string(),
			));
		}
	}

	let autovacuum_max_workers = if budget.cpus >= 32 {
		Some(5)
	} else if budget.cpus >= 16 {
		Some(4)
	} else {
		None
	};
	if let Some(workers) = autovacuum_max_workers {
		out.push(Setting::new("autovacuum_max_workers", workers.to_string()));
	}
	if expected_maintenance_kib(&budget, platform, pg_major) >= 2 * GIB {
		out.push(Setting::new("autovacuum_work_mem", "2GB"));
	}

	if pg_major >= 18 {
		let io_workers = (budget.cpus / 4).clamp(3, 32);
		if io_workers > 3 {
			out.push(Setting::new("io_workers", io_workers.to_string()));
		}
		// io_uring is Linux-only; leave the default worker method on Windows.
		if !platform.is_windows() {
			out.push(Setting::new("io_method", "io_uring"));
		}
	}

	if pg_major >= 12 {
		out.push(Setting::new("jit", "off"));
	}

	let wal_compression = if pg_major >= 15 {
		if lz4_wal_supported {
			Some("lz4")
		} else {
			Some("on")
		}
	} else if pg_major >= 10 {
		Some("on")
	} else {
		None
	};
	if let Some(value) = wal_compression {
		out.push(Setting::new("wal_compression", value));
	}

	if let Some(kib) = temp_file_limit_kib {
		out.push(Setting::new("temp_file_limit", render_kib(kib)));
	}

	out
}

#[cfg(test)]
mod tests {
	use super::*;

	fn get<'a>(settings: &'a [Setting], key: &str) -> Option<&'a str> {
		settings
			.iter()
			.find(|s| s.key == key)
			.map(|s| s.value.as_str())
	}

	fn windows_inputs(total_gib: u64, cpus: u32) -> TuneInputs {
		TuneInputs {
			platform: Platform::Windows,
			resources: HostResources {
				total_ram_kib: total_gib * GIB,
				cpus,
			},
			pg_major: 16,
			max_connections: 100,
			lz4_wal_supported: false,
			temp_file_limit_kib: None,
		}
	}

	#[test]
	fn windows_budget_is_capped_at_4gib() {
		assert_eq!(budget_ram_kib(Platform::Windows, 4 * GIB), 2 * GIB);
		assert_eq!(budget_ram_kib(Platform::Windows, 8 * GIB), 4 * GIB);
		assert_eq!(budget_ram_kib(Platform::Windows, 16 * GIB), 4 * GIB);
		assert_eq!(budget_ram_kib(Platform::Windows, 64 * GIB), 4 * GIB);
	}

	#[test]
	fn linux_budget_reserves_four_gib() {
		assert_eq!(budget_ram_kib(Platform::Linux, 32 * GIB), 28 * GIB);
		assert_eq!(budget_ram_kib(Platform::Linux, 4 * GIB), 2 * GIB);
	}

	#[test]
	fn windows_shared_buffers_scale_with_budget() {
		assert_eq!(
			get(&compute(&windows_inputs(4, 2)), "shared_buffers"),
			Some("512MB")
		);
		assert_eq!(
			get(&compute(&windows_inputs(8, 2)), "shared_buffers"),
			Some("1GB")
		);
		assert_eq!(
			get(&compute(&windows_inputs(16, 4)), "shared_buffers"),
			Some("1GB")
		);
		assert_eq!(
			get(&compute(&windows_inputs(64, 16)), "shared_buffers"),
			Some("1GB")
		);
	}

	#[test]
	fn windows_omits_linux_only_settings() {
		let s = compute(&windows_inputs(16, 8));
		assert_eq!(get(&s, "effective_io_concurrency"), None);
		assert_eq!(get(&s, "io_method"), None);
	}

	#[test]
	fn windows_16gib_core_values() {
		let s = compute(&windows_inputs(16, 4));
		// budget = 4GiB
		assert_eq!(get(&s, "effective_cache_size"), Some("3GB"));
		assert_eq!(get(&s, "maintenance_work_mem"), Some("256MB"));
		assert_eq!(get(&s, "max_wal_size"), Some("8GB"));
		assert_eq!(get(&s, "random_page_cost"), Some("1.1"));
		assert_eq!(get(&s, "huge_pages"), Some("off"));
	}

	#[test]
	fn parallelism_gated_on_cpu_budget() {
		// 2 CPUs -> budget 2 -> no parallel settings.
		let s = compute(&windows_inputs(16, 2));
		assert_eq!(get(&s, "max_worker_processes"), None);
		// 8 CPUs -> budget 4 -> parallel settings present.
		let s = compute(&windows_inputs(16, 8));
		assert_eq!(get(&s, "max_worker_processes"), Some("4"));
		assert_eq!(get(&s, "max_parallel_workers_per_gather"), Some("2"));
		assert_eq!(get(&s, "max_parallel_workers"), Some("4"));
		assert_eq!(get(&s, "max_parallel_maintenance_workers"), Some("2"));
	}

	#[test]
	fn wal_compression_falls_back_without_lz4() {
		let mut input = windows_inputs(16, 4);
		input.pg_major = 16;
		input.lz4_wal_supported = false;
		assert_eq!(get(&compute(&input), "wal_compression"), Some("on"));
		input.lz4_wal_supported = true;
		assert_eq!(get(&compute(&input), "wal_compression"), Some("lz4"));
		input.pg_major = 13;
		input.lz4_wal_supported = true;
		assert_eq!(get(&compute(&input), "wal_compression"), Some("on"));
	}

	#[test]
	fn pre_pg10_windows_caps_shared_buffers() {
		let mut input = windows_inputs(64, 8);
		input.pg_major = 9;
		// budget 4GiB -> /4 = 1GiB, capped to 512MiB pre-10.
		assert_eq!(get(&compute(&input), "shared_buffers"), Some("512MB"));
		// no jit / wal_compression pre-10/12
		assert_eq!(get(&compute(&input), "jit"), None);
		assert_eq!(get(&compute(&input), "wal_compression"), None);
	}

	#[test]
	fn temp_file_limit_emitted_when_present() {
		let mut input = windows_inputs(16, 4);
		input.temp_file_limit_kib = Some(50 * GIB);
		assert_eq!(get(&compute(&input), "temp_file_limit"), Some("50GB"));
		input.temp_file_limit_kib = None;
		assert_eq!(get(&compute(&input), "temp_file_limit"), None);
	}

	#[test]
	fn linux_32gib_matches_ops_tuning() {
		let input = TuneInputs {
			platform: Platform::Linux,
			resources: HostResources {
				total_ram_kib: 32 * GIB,
				cpus: 8,
			},
			pg_major: 16,
			max_connections: 100,
			lz4_wal_supported: true,
			temp_file_limit_kib: None,
		};
		let s = compute(&input);
		// budget = 28GiB
		assert_eq!(get(&s, "shared_buffers"), Some("7GB"));
		assert_eq!(get(&s, "effective_cache_size"), Some("21GB"));
		assert_eq!(get(&s, "random_page_cost"), Some("1.1"));
		assert_eq!(get(&s, "effective_io_concurrency"), Some("200"));
		assert_eq!(get(&s, "max_wal_size"), Some("8GB"));
		assert_eq!(get(&s, "wal_compression"), Some("lz4"));
	}

	#[test]
	fn render_kib_uses_largest_exact_unit() {
		assert_eq!(render_kib(GIB), "1GB");
		assert_eq!(render_kib(512 * MIB), "512MB");
		assert_eq!(render_kib(32), "32kB");
		assert_eq!(render_kib(1536 * MIB), "1536MB");
	}
}

//! Non-check facts attached to every doctor run: OS, virtualisation,
//! filesystems, network capability probes.

use std::{
	collections::BTreeMap,
	io,
	net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
	path::Path,
	time::Duration,
};

use serde::Serialize;
use sysinfo::{CpuRefreshKind, Disks, MemoryRefreshKind, RefreshKind, System};
use tokio::net::TcpStream;
use tracing::debug;
use url::Url;

use bestool_tamanu::server_info::{detect_node_version, detect_virtualisation};

const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

/// Link-local IMDS endpoint on EC2. Reachable only from within an instance.
const IMDS_BASE: &str = "http://169.254.169.254/latest";
/// Kept short: off-EC2 the address is unroutable, so every request must fail
/// fast rather than stalling the whole sweep.
const IMDS_TIMEOUT: Duration = Duration::from_secs(2);
const IPV4_PROBE_ADDR: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)), 443);
const IPV6_PROBE_ADDR: SocketAddr = SocketAddr::new(
	IpAddr::V6(Ipv6Addr::new(0x2606, 0x4700, 0x4700, 0, 0, 0, 0, 0x1111)),
	443,
);
const NAT64_PROBE_HOST: &str = "ipv4only.arpa";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Filesystem {
	pub mountpoint: String,
	pub fs_type: String,
}

/// The machine's own facts: the box, its operating system, its hardware, its
/// network identity, and the agent reporting on it.
///
/// Field names match (and extend) the previous `SendStatusToMetaServer` shape
/// in Tamanu's `packages/shared/src/tasks/SendStatusToMetaServer.js`, so
/// downstream parsing of historic rows stays compatible.
///
/// spec: SUBJ
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MachineInfo {
	/// The version of bestool running on this machine. A machine fact: it
	/// answers whether the agent here needs upgrading, and an application with
	/// no agent alongside it has none.
	pub bestool_version: String,
	pub hostname: Option<String>,
	/// OS-level system timezone. Distinct from an application's configured
	/// zone, which is reported against the application; drift between the two
	/// is still gradable wherever one sweep holds both.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub os_timezone: Option<String>,

	pub uptime_secs: u64,
	/// Logical CPU count (what load average is relative to, i.e. `nproc`).
	pub cpu_cores: usize,
	/// Total physical memory, in bytes.
	pub total_memory_bytes: u64,
	pub os_kind: &'static str,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub os_name: Option<String>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub os_version: Option<String>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub kernel: Option<String>,
	pub arch: String,
	/// Whether this host is a guest: `true` virtualised, `false` bare metal,
	/// `null` when detection came up empty. Always serialised, including the
	/// null — an absent answer is a distinct fact from a negative one, and
	/// collapsing the two is how Windows hosts spent so long looking physical.
	pub virtualised: Option<bool>,
	/// The hypervisor, in `systemd-detect-virt`'s vocabulary (`microsoft`,
	/// `amazon`, `vmware`, `kvm`, …), or `none` on bare metal. Absent when we
	/// couldn't tell either way.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub virtualisation: Option<String>,
	pub filesystems: Vec<Filesystem>,
	pub ipv4: bool,
	pub ipv6: bool,
	pub nat64: bool,
	/// EC2 instance tags read from IMDS, when running on an AWS instance that
	/// exposes them. Absent off-EC2 or when instance metadata tags aren't
	/// enabled for the instance.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub instance_tags: Option<BTreeMap<String, String>>,
}

/// One application's own facts.
///
/// Every field is about the application rather than the box under it, so an
/// application reports no `bestoolVersion`, no hostname, and none of the
/// machine's hardware.
///
/// spec: SUBJ
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplicationInfo {
	/// Version of the deployment. Absent when it could not be resolved from
	/// either the install or the database.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub tamanu_version: Option<String>,
	/// Whether this application is a `central` or `facility`, when known.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub tamanu_server_kind: Option<&'static str>,
	/// Filesystem root of the install, when one was found on disk. Absent on
	/// hosts driven by `TAMANU_DATABASE_URL` alone (no install).
	#[serde(skip_serializing_if = "Option::is_none")]
	pub tamanu_root: Option<String>,
	/// Version of the Node.js runtime the application executes under (bare, no
	/// leading `v`). Omitted when no runtime could be resolved.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub node_version: Option<String>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub canonical_url: Option<String>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub current_sync_tick: Option<String>,
	/// Effective timezone in use by the application (from `primaryTimeZone` /
	/// `countryTimeZone` config).
	#[serde(skip_serializing_if = "Option::is_none")]
	pub timezone: Option<String>,
}

/// The Postgres installation's own facts.
///
/// The server's version is its own, not that of whatever connects to it, so it
/// is reported here rather than against each application using the database.
///
/// spec: SUBJ
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PostgresInfo {
	#[serde(skip_serializing_if = "Option::is_none")]
	pub pg_version: Option<String>,
}

/// Optional inputs sourced from the Tamanu DB / config that aren't trivially
/// available at gather time. Doctor populates these from its own DB connection.
#[derive(Debug, Clone, Default)]
pub struct ServerFacts {
	pub canonical_url: Option<String>,
	pub current_sync_tick: Option<String>,
	pub timezone: Option<String>,
	pub pg_version: Option<String>,
	pub tamanu_root: Option<String>,
	pub tamanu_server_kind: Option<&'static str>,
}

/// Build the machine's, the application's, and Postgres's fact blocks.
///
/// The two are gathered together because one pass over the host answers both,
/// but nothing crosses between them: each field lands on the subject it is
/// actually about, so a fact is absent when its subject genuinely lacks it.
/// The application block is meaningful only when the host has an application;
/// on a machine with none the caller discards it.
///
/// `bestool_version` is the version of the *calling* binary — it must be
/// provided by the caller (`env!("CARGO_PKG_VERSION")` resolved in the bestool
/// crate) rather than evaluated here, since in this library crate it would
/// resolve to the library's own version instead of the running binary's.
///
/// spec: SUBJ
pub async fn gather(
	bestool_version: &str,
	tamanu_version: Option<String>,
	facts: ServerFacts,
) -> (MachineInfo, ApplicationInfo, PostgresInfo) {
	let disks = Disks::new_with_refreshed_list();
	let filesystems = disks
		.iter()
		.map(|d| Filesystem {
			mountpoint: d.mount_point().to_string_lossy().to_string(),
			fs_type: d.file_system().to_string_lossy().to_string(),
		})
		.collect();

	let virt = detect_virtualisation().await;
	// No answer stays no answer: `None` here means "couldn't tell", never "bare
	// metal".
	let virtualised = virt.as_deref().map(|virt| virt != "none");

	let (ipv4, ipv6, nat64, instance_tags) =
		futures::join!(probe_ipv4(), probe_ipv6(), probe_nat64(), fetch_imds_tags());

	let os_timezone = jiff::tz::TimeZone::system()
		.iana_name()
		.map(|s| s.to_string());

	let sys = System::new_with_specifics(
		RefreshKind::nothing()
			.with_cpu(CpuRefreshKind::nothing())
			.with_memory(MemoryRefreshKind::nothing().with_ram()),
	);
	let cpu_cores = sys.cpus().len();
	let total_memory_bytes = sys.total_memory();

	let node_version = detect_node_version(
		facts.tamanu_root.as_deref().map(Path::new),
		tamanu_version.as_deref(),
	)
	.await;

	let application = ApplicationInfo {
		tamanu_version,
		tamanu_server_kind: facts.tamanu_server_kind,
		tamanu_root: facts.tamanu_root,
		node_version,
		canonical_url: facts.canonical_url,
		current_sync_tick: facts.current_sync_tick,
		timezone: facts.timezone,
	};

	let postgres = PostgresInfo {
		pg_version: facts.pg_version,
	};

	let machine = MachineInfo {
		bestool_version: bestool_version.to_string(),
		hostname: System::host_name(),
		os_timezone,
		uptime_secs: System::uptime(),
		cpu_cores,
		total_memory_bytes,
		os_kind: if cfg!(target_os = "linux") {
			"linux"
		} else if cfg!(target_os = "windows") {
			"windows"
		} else if cfg!(target_os = "macos") {
			"macos"
		} else {
			"other"
		},
		os_name: System::name(),
		os_version: System::os_version().or_else(System::long_os_version),
		kernel: System::kernel_version(),
		arch: std::env::consts::ARCH.to_string(),
		virtualised,
		virtualisation: virt,
		filesystems,
		ipv4,
		ipv6,
		nat64,
		instance_tags,
	};

	(machine, application, postgres)
}

/// Read EC2 instance tags via IMDSv2.
///
/// Uses the token-authenticated v2 flow: `PUT /api/token` then reads the tag
/// keys from `/meta-data/tags/instance` and fetches each value. Returns `None`
/// when not on EC2, when the token endpoint is unreachable, or when instance
/// metadata tags aren't enabled for the instance — this is best-effort host
/// context and never fails the sweep.
pub(crate) async fn fetch_imds_tags() -> Option<BTreeMap<String, String>> {
	let client = reqwest::Client::builder()
		.timeout(IMDS_TIMEOUT)
		.connect_timeout(IMDS_TIMEOUT)
		.build()
		.ok()?;

	let token = client
		.put(format!("{IMDS_BASE}/api/token"))
		.header("X-aws-ec2-metadata-token-ttl-seconds", "60")
		.send()
		.await
		.ok()?
		.error_for_status()
		.ok()?
		.text()
		.await
		.ok()?;

	let list = client
		.get(format!("{IMDS_BASE}/meta-data/tags/instance"))
		.header("X-aws-ec2-metadata-token", &token)
		.send()
		.await
		.ok()?
		.error_for_status()
		.ok()?
		.text()
		.await
		.ok()?;

	let keys = parse_tag_keys(&list);
	if keys.is_empty() {
		return None;
	}

	let mut tags = BTreeMap::new();
	for key in keys {
		let mut url = match Url::parse(&format!("{IMDS_BASE}/meta-data/tags/instance")) {
			Ok(url) => url,
			Err(err) => {
				debug!(%err, "could not build IMDS tag URL");
				continue;
			}
		};
		// `push` percent-encodes the segment, so tag keys with `aws:` prefixes
		// or other reserved characters resolve to the right path.
		match url.path_segments_mut() {
			Ok(mut segments) => {
				segments.push(&key);
			}
			Err(()) => continue,
		}

		match client
			.get(url)
			.header("X-aws-ec2-metadata-token", &token)
			.send()
			.await
			.and_then(|r| r.error_for_status())
		{
			Ok(resp) => match resp.text().await {
				Ok(value) => {
					tags.insert(key, value);
				}
				Err(err) => debug!(%key, %err, "could not read IMDS tag value"),
			},
			Err(err) => debug!(%key, %err, "could not fetch IMDS tag"),
		}
	}

	if tags.is_empty() { None } else { Some(tags) }
}

/// Parse the newline-separated tag key listing IMDS returns from
/// `/meta-data/tags/instance`, dropping blank lines.
fn parse_tag_keys(list: &str) -> Vec<String> {
	list.lines()
		.map(str::trim)
		.filter(|line| !line.is_empty())
		.map(String::from)
		.collect()
}

async fn probe_tcp(addr: SocketAddr) -> bool {
	match tokio::time::timeout(PROBE_TIMEOUT, TcpStream::connect(addr)).await {
		Ok(Ok(_)) => true,
		Ok(Err(err)) => {
			debug!(?addr, %err, "tcp probe failed");
			false
		}
		Err(_) => {
			debug!(?addr, "tcp probe timed out");
			false
		}
	}
}

async fn probe_ipv4() -> bool {
	probe_tcp(IPV4_PROBE_ADDR).await
}

async fn probe_ipv6() -> bool {
	probe_tcp(IPV6_PROBE_ADDR).await
}

/// True if a NAT64 prefix is in use on the network: a system AAAA lookup for
/// `ipv4only.arpa` (an A-only name) returns a synthesised AAAA address.
async fn probe_nat64() -> bool {
	let result = tokio::time::timeout(PROBE_TIMEOUT, resolve_aaaa(NAT64_PROBE_HOST)).await;
	match result {
		Ok(Ok(present)) => present,
		Ok(Err(err)) => {
			debug!(%err, "nat64 probe failed");
			false
		}
		Err(_) => {
			debug!("nat64 probe timed out");
			false
		}
	}
}

async fn resolve_aaaa(host: &str) -> io::Result<bool> {
	use hickory_resolver::TokioResolver;

	let resolver = TokioResolver::builder_tokio()
		.map_err(io::Error::other)?
		.build()
		.map_err(io::Error::other)?;
	let response = resolver.ipv6_lookup(host).await.map_err(io::Error::other)?;
	Ok(!response.answers().is_empty())
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn parse_tag_keys_splits_and_trims_lines() {
		let list = "Name\nenvironment\naws:cloudformation:stack-name\n";
		assert_eq!(
			parse_tag_keys(list),
			vec![
				"Name".to_string(),
				"environment".to_string(),
				"aws:cloudformation:stack-name".to_string(),
			]
		);
	}

	#[test]
	fn parse_tag_keys_drops_blank_lines() {
		assert!(parse_tag_keys("").is_empty());
		assert!(parse_tag_keys("\n  \n\n").is_empty());
		assert_eq!(parse_tag_keys("\nName\n\n"), vec!["Name".to_string()]);
	}
}

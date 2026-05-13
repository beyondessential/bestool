//! Non-check facts attached to every doctor run: OS, virtualisation,
//! filesystems, network capability probes.

use std::{
	io,
	net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
	time::Duration,
};

use serde::Serialize;
use sysinfo::{Disks, System};
use tokio::net::TcpStream;
use tracing::debug;

use crate::actions::tamanu::server_info::detect_virtualisation;

const PROBE_TIMEOUT: Duration = Duration::from_secs(3);
const IPV4_PROBE_ADDR: SocketAddr =
	SocketAddr::new(IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)), 443);
const IPV6_PROBE_ADDR: SocketAddr = SocketAddr::new(
	IpAddr::V6(Ipv6Addr::new(0x2606, 0x4700, 0x4700, 0, 0, 0, 0, 0x1111)),
	443,
);
const NAT64_PROBE_HOST: &str = "ipv4only.arpa";

#[derive(Debug, Clone, Serialize)]
pub struct Filesystem {
	pub mountpoint: String,
	pub fs_type: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ServerInfo {
	pub bestool_version: &'static str,
	pub tamanu_version: String,
	pub hostname: Option<String>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub canonical_url: Option<String>,
	pub uptime_secs: u64,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub timezone: Option<String>,
	pub os_kind: &'static str,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub os_name: Option<String>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub os_version: Option<String>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub kernel: Option<String>,
	pub arch: String,
	pub virtualised: bool,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub virtualisation: Option<String>,
	pub filesystems: Vec<Filesystem>,
	pub ipv4: bool,
	pub ipv6: bool,
	pub nat64: bool,
}

pub async fn gather(tamanu_version: &str) -> ServerInfo {
	let disks = Disks::new_with_refreshed_list();
	let filesystems = disks
		.iter()
		.map(|d| Filesystem {
			mountpoint: d.mount_point().to_string_lossy().to_string(),
			fs_type: d.file_system().to_string_lossy().to_string(),
		})
		.collect();

	let virt = detect_virtualisation();
	let virtualised = !matches!(virt.as_deref(), None | Some("none"));

	let (ipv4, ipv6, nat64) = futures::join!(probe_ipv4(), probe_ipv6(), probe_nat64());

	ServerInfo {
		bestool_version: env!("CARGO_PKG_VERSION"),
		tamanu_version: tamanu_version.to_string(),
		hostname: System::host_name(),
		canonical_url: None,
		uptime_secs: System::uptime(),
		timezone: jiff::tz::TimeZone::system()
			.iana_name()
			.map(|s| s.to_string()),
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
	}
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
		.build();
	let response = resolver
		.ipv6_lookup(host)
		.await
		.map_err(io::Error::other)?;
	Ok(response.iter().next().is_some())
}


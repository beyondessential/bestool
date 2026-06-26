//! Reports the host's network addresses as status facts: the LAN IPv4/IPv6
//! addresses, and a best-guess public (WAN) IP.
//!
//! "LAN" is the `scope global` equivalent — every address that isn't loopback or
//! link-local — minus the interfaces that aren't really the LAN: tailscale and
//! container/bridge interfaces (podman, docker, cni, veth, …). Tailscale's
//! address ranges are dropped too, belt-and-suspenders, in case its interface is
//! named oddly.
//!
//! The WAN lookup hits an external service, so it's cached to a file and only
//! refreshed once an hour; frequent sweeps reuse the cached value (and keep the
//! last known one if a refresh fails). These are facts for the top-level status
//! payload (like `osTimezone`), not a health signal, so the check is always a
//! pass and carries them in `payload_extras`.

use std::{
	net::IpAddr,
	path::{Path, PathBuf},
	time::Duration,
};

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sysinfo::Networks;

use super::SweepContext;
use crate::doctor::check::Check;

const NAME: &str = "ips";

/// Re-query the WAN service at most this often; sweeps in between reuse the cache.
const WAN_REFRESH_SECS: i64 = 60 * 60;

/// Per-request timeout for a WAN lookup.
const WAN_TIMEOUT: Duration = Duration::from_secs(5);

/// External services raced for the public IP. The family-specific hostnames
/// force the address family via DNS (A-only / AAAA-only); `ifconfig.me` has no
/// split host, so its response is validated against the wanted family instead.
const WAN_V4_URLS: &[&str] = &[
	"https://ipv4.icanhazip.com",
	"https://api4.ipify.org",
	"https://ifconfig.me/ip",
];
const WAN_V6_URLS: &[&str] = &[
	"https://ipv6.icanhazip.com",
	"https://api6.ipify.org",
	"https://ifconfig.me/ip",
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Family {
	V4,
	V6,
}

pub async fn run(ctx: SweepContext) -> Check {
	let lan = lan_addresses();
	let wan = wan_addresses(&ctx.http_client).await;

	let mut check = Check::pass(NAME, summarise(&lan, &wan)).with_payload_extra(
		"lanIps",
		json!(lan.iter().map(IpAddr::to_string).collect::<Vec<_>>()),
	);
	if let Some(v4) = wan.v4 {
		check = check.with_payload_extra("wanIpv4", v4.to_string());
	}
	if let Some(v6) = wan.v6 {
		check = check.with_payload_extra("wanIpv6", v6.to_string());
	}
	check
}

/// The LAN addresses: every interface address that survives [`is_lan_address`].
fn lan_addresses() -> Vec<IpAddr> {
	let networks = Networks::new_with_refreshed_list();
	let mut out: Vec<IpAddr> = Vec::new();
	for (interface, data) in &networks {
		for net in data.ip_networks() {
			if is_lan_address(interface, net.addr) && !out.contains(&net.addr) {
				out.push(net.addr);
			}
		}
	}
	out.sort();
	out
}

/// Whether an interface address counts as a LAN address: globally-scoped (not
/// loopback or link-local), on an interface that's a real NIC rather than a
/// tailscale or container/bridge interface, and not in a tailscale range.
fn is_lan_address(interface: &str, addr: IpAddr) -> bool {
	!addr.is_loopback()
		&& !is_link_local(addr)
		&& !is_excluded_interface(interface)
		&& !is_tailscale_address(addr)
}

fn is_link_local(addr: IpAddr) -> bool {
	match addr {
		IpAddr::V4(v4) => v4.is_link_local(),
		// fe80::/10
		IpAddr::V6(v6) => (v6.segments()[0] & 0xffc0) == 0xfe80,
	}
}

/// Interfaces that aren't the host's LAN: tailscale, and container / VM / VPN
/// bridges and virtual links (matched by name prefix, case-insensitively).
fn is_excluded_interface(name: &str) -> bool {
	const PREFIXES: &[&str] = &[
		"tailscale",
		"podman",
		"cni",
		"docker",
		"br-",
		"virbr",
		"veth",
		"wg",
		"zt",
		"tun",
		"tap",
		"kube",
		"flannel",
		"cali",
		"nerdctl",
		"vmnet",
		"vethernet",
		"wsl",
	];
	let name = name.to_ascii_lowercase();
	name == "lo" || PREFIXES.iter().any(|prefix| name.starts_with(prefix))
}

/// Tailscale's address ranges: the `100.64.0.0/10` CGNAT block and the
/// `fd7a:115c:a1e0::/48` ULA prefix it carves its IPv6 out of.
fn is_tailscale_address(addr: IpAddr) -> bool {
	match addr {
		IpAddr::V4(v4) => {
			let [a, b, ..] = v4.octets();
			a == 100 && (64..=127).contains(&b)
		}
		IpAddr::V6(v6) => {
			let s = v6.segments();
			s[0] == 0xfd7a && s[1] == 0x115c && s[2] == 0xa1e0
		}
	}
}

struct WanAddresses {
	v4: Option<IpAddr>,
	v6: Option<IpAddr>,
}

/// The best-guess public IPs, from the cache when fresh, else re-queried (racing
/// the external services per family) and re-cached. A failed refresh keeps the
/// last known value rather than dropping it.
async fn wan_addresses(client: &reqwest::Client) -> WanAddresses {
	let path = cache_path();
	let cache = read_cache(&path).await;

	if cache.checked_at.is_some_and(is_fresh) {
		return WanAddresses {
			v4: parse_ip(cache.ipv4.as_deref()),
			v6: parse_ip(cache.ipv6.as_deref()),
		};
	}

	let v4 = race(client, WAN_V4_URLS, Family::V4)
		.await
		.or_else(|| parse_ip(cache.ipv4.as_deref()));
	let v6 = race(client, WAN_V6_URLS, Family::V6)
		.await
		.or_else(|| parse_ip(cache.ipv6.as_deref()));

	write_cache(
		&path,
		&WanCache {
			checked_at: Some(Timestamp::now()),
			ipv4: v4.map(|ip| ip.to_string()),
			ipv6: v6.map(|ip| ip.to_string()),
		},
	)
	.await;

	WanAddresses { v4, v6 }
}

/// Race the services for `family`, returning the first valid response.
async fn race(client: &reqwest::Client, urls: &[&str], family: Family) -> Option<IpAddr> {
	let attempts = urls
		.iter()
		.map(|&url| Box::pin(fetch_ip(client, url, family)));
	futures::future::select_ok(attempts)
		.await
		.ok()
		.map(|(ip, _)| ip)
}

async fn fetch_ip(client: &reqwest::Client, url: &str, family: Family) -> Result<IpAddr, ()> {
	let body = client
		.get(url)
		.timeout(WAN_TIMEOUT)
		.send()
		.await
		.map_err(|_| ())?
		.text()
		.await
		.map_err(|_| ())?;
	let ip: IpAddr = body.trim().parse().map_err(|_| ())?;
	matches_family(family, ip).then_some(ip).ok_or(())
}

fn matches_family(family: Family, ip: IpAddr) -> bool {
	matches!(
		(family, ip),
		(Family::V4, IpAddr::V4(_)) | (Family::V6, IpAddr::V6(_))
	)
}

#[derive(Default, Serialize, Deserialize)]
struct WanCache {
	checked_at: Option<Timestamp>,
	ipv4: Option<String>,
	ipv6: Option<String>,
}

fn is_fresh(checked_at: Timestamp) -> bool {
	Timestamp::now().as_second() - checked_at.as_second() < WAN_REFRESH_SECS
}

fn parse_ip(s: Option<&str>) -> Option<IpAddr> {
	s.and_then(|s| s.parse().ok())
}

fn cache_path() -> PathBuf {
	dirs::cache_dir()
		.unwrap_or_else(std::env::temp_dir)
		.join("bestool")
		.join("wan-ip.json")
}

async fn read_cache(path: &Path) -> WanCache {
	match tokio::fs::read(path).await {
		Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
		Err(_) => WanCache::default(),
	}
}

async fn write_cache(path: &Path, cache: &WanCache) {
	if let Some(parent) = path.parent() {
		let _ = tokio::fs::create_dir_all(parent).await;
	}
	if let Ok(bytes) = serde_json::to_vec(cache) {
		let _ = tokio::fs::write(path, bytes).await;
	}
}

fn summarise(lan: &[IpAddr], wan: &WanAddresses) -> String {
	let lan = if lan.is_empty() {
		"none".to_string()
	} else {
		lan.iter()
			.map(IpAddr::to_string)
			.collect::<Vec<_>>()
			.join(", ")
	};
	let wan: Vec<String> = [wan.v4, wan.v6]
		.into_iter()
		.flatten()
		.map(|ip| ip.to_string())
		.collect();
	let wan = if wan.is_empty() {
		"unknown".to_string()
	} else {
		wan.join(", ")
	};
	format!("LAN {lan}; WAN {wan}")
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn keeps_the_real_nic_address() {
		assert!(is_lan_address("ens33", "10.11.3.7".parse().unwrap()));
	}

	#[test]
	fn drops_loopback_and_link_local() {
		assert!(!is_lan_address("lo", "127.0.0.1".parse().unwrap()));
		assert!(!is_lan_address("lo", "::1".parse().unwrap()));
		assert!(!is_lan_address(
			"ens33",
			"fe80::250:56ff:fea8:1133".parse().unwrap()
		));
		assert!(!is_lan_address("ens33", "169.254.1.1".parse().unwrap()));
	}

	#[test]
	fn drops_tailscale_by_name_and_by_range() {
		// tailscale0's globally-scoped addresses must not count as LAN.
		assert!(!is_lan_address(
			"tailscale0",
			"100.67.55.52".parse().unwrap()
		));
		assert!(!is_lan_address(
			"tailscale0",
			"fd7a:115c:a1e0::8501:379d".parse().unwrap()
		));
		// …even if the interface were named otherwise (range catches it).
		assert!(!is_lan_address("eth9", "100.100.0.1".parse().unwrap()));
		assert!(is_tailscale_address("100.64.0.1".parse().unwrap()));
		assert!(!is_tailscale_address("100.128.0.1".parse().unwrap()));
	}

	#[test]
	fn drops_podman_bridge_though_it_is_scope_global() {
		// podman0 carries a globally-scoped 10.100.0.1/16; excluded by name.
		assert!(!is_lan_address("podman0", "10.100.0.1".parse().unwrap()));
		assert!(!is_lan_address("veth0", "10.0.0.5".parse().unwrap()));
		assert!(!is_lan_address("docker0", "172.17.0.1".parse().unwrap()));
		assert!(!is_lan_address(
			"vEthernet (WSL)",
			"172.20.0.1".parse().unwrap()
		));
	}

	#[test]
	fn family_validation() {
		assert!(matches_family(Family::V4, "1.2.3.4".parse().unwrap()));
		assert!(!matches_family(Family::V4, "::1".parse().unwrap()));
		assert!(matches_family(Family::V6, "2001:db8::1".parse().unwrap()));
		assert!(!matches_family(Family::V6, "1.2.3.4".parse().unwrap()));
	}
}

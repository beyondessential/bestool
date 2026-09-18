//! Who is asking for a certificate.
//!
//! The endpoint hands out a private key, so it identifies its caller rather than
//! trusting it for being local. Peer credentials over a socket are not reachable
//! through Caddy: `HTTPCertGetter` parses the configured URL with `url.Parse`
//! and issues the request through `http.DefaultClient` — no unix socket, no
//! custom dialer, no client certificate, no configurable transport. So the
//! connection is ordinary loopback TCP and the peer is resolved out of band.
//!
//! On Linux the kernel already records the owner of every socket in
//! `/proc/net/tcp` and `/proc/net/tcp6`, so matching the connection's four-tuple
//! there gives the user directly. Walking `/proc/*/fd` to find the holding
//! process would name the process too, but it needs `CAP_SYS_PTRACE` to read
//! another user's process — which the daemon's unit deliberately does not grant,
//! and which would have to be added for no gain: a socket's owner is the
//! effective user of whatever created it, which is the thing being decided on.
//!
//! Windows needs its own lookup (`GetExtendedTcpTable`) and its own user model,
//! and is carried by its own card. Until then this refuses on any other
//! platform, which is the safe direction: a refusal is a misconfiguration to
//! correct, not a silent grant.
//!
//! spec: TLSD#who-may-fetch-a-certificate

use std::net::SocketAddr;

use miette::{Result, miette};

/// The superuser, who may always fetch a certificate.
const ROOT_UID: u32 = 0;

/// Who a caller is permitted to be, beyond the superuser.
///
/// Follows tailscaled's shape (`TS_PERMIT_CERT_UID=caddy`), which exists for
/// exactly this: Caddy commonly runs as its own unprivileged user.
#[derive(Debug, Clone, Copy, Default)]
pub struct Permitted {
	/// One further user, named in the daemon's configuration.
	pub uid: Option<u32>,
}

impl Permitted {
	pub fn new(uid: Option<u32>) -> Self {
		Self { uid }
	}

	fn admits(&self, uid: u32) -> bool {
		uid == ROOT_UID || self.uid == Some(uid)
	}
}

/// Resolve a configured user — a name or a numeric id — to a uid.
///
/// A name is looked up in `/etc/passwd`. That misses a user served only by NSS
/// (LDAP, and the like), which a service account for a front end is not; a
/// numeric id is the way through for one that is.
pub fn resolve_user(spec: &str) -> Result<u32> {
	let spec = spec.trim();
	if let Ok(uid) = spec.parse::<u32>() {
		return Ok(uid);
	}

	let passwd = std::fs::read_to_string("/etc/passwd")
		.map_err(|err| miette!("could not read /etc/passwd to resolve {spec:?}: {err}"))?;
	passwd
		.lines()
		.filter_map(|line| {
			let mut fields = line.split(':');
			let name = fields.next()?;
			let uid = fields.nth(1)?.parse::<u32>().ok()?;
			(name == spec).then_some(uid)
		})
		.next()
		.ok_or_else(|| miette!("no user {spec:?} on this host; give a numeric uid instead"))
}

/// Whether the peer of a connection may fetch a certificate.
///
/// `peer` is the far end of the connection as the daemon sees it, and `local`
/// the end it accepted on: together they name the caller's socket in the
/// kernel's table.
#[cfg(target_os = "linux")]
pub fn caller_permitted(permitted: Permitted, peer: SocketAddr, local: SocketAddr) -> Result<()> {
	let uid = socket_owner(peer, local)?;
	if permitted.admits(uid) {
		Ok(())
	} else {
		Err(miette!(
			"uid {uid} may not fetch a certificate; configure --permit-cert-user if this is the user the front end runs as"
		))
	}
}

#[cfg(not(target_os = "linux"))]
pub fn caller_permitted(
	_permitted: Permitted,
	_peer: SocketAddr,
	_local: SocketAddr,
) -> Result<()> {
	Err(miette!(
		"identifying the caller of the certificate endpoint is not implemented on this platform"
	))
}

/// The uid owning the socket at the far end of this connection.
///
/// The caller's socket is the mirror of ours: its local address is our peer's,
/// and its remote address is the one we accepted on.
#[cfg(target_os = "linux")]
fn socket_owner(peer: SocketAddr, local: SocketAddr) -> Result<u32> {
	for table in ["/proc/net/tcp", "/proc/net/tcp6"] {
		let Ok(body) = std::fs::read_to_string(table) else {
			continue;
		};
		if let Some(uid) = find_owner(&body, peer, local) {
			return Ok(uid);
		}
	}
	Err(miette!(
		"could not find the caller's socket ({peer} -> {local}) in the kernel's table"
	))
}

/// Find the uid owning the socket whose local end is `want_local` and whose
/// remote end is `want_remote`, in one of the kernel's TCP tables.
///
/// The table is a fixed-column text file: a header line, then one line per
/// socket with `local_address` and `rem_address` as hex `ADDRESS:PORT`, and the
/// owner's uid in the eighth field.
fn find_owner(table: &str, want_local: SocketAddr, want_remote: SocketAddr) -> Option<u32> {
	table.lines().skip(1).find_map(|line| {
		let mut fields = line.split_whitespace();
		let _slot = fields.next()?;
		let local = parse_address(fields.next()?)?;
		let remote = parse_address(fields.next()?)?;
		// st, tx:rx, tr:tm, retrnsmt, then uid.
		let uid = fields.nth(4)?.parse::<u32>().ok()?;

		(same_endpoint(local, want_local) && same_endpoint(remote, want_remote)).then_some(uid)
	})
}

/// Whether two endpoints name the same socket, treating an IPv4-mapped IPv6
/// address as the IPv4 address it maps.
///
/// A connection to `127.0.0.1` on a dual-stack listener can appear in either
/// table, and in `tcp6` it appears mapped.
fn same_endpoint(a: SocketAddr, b: SocketAddr) -> bool {
	fn canonical(addr: SocketAddr) -> SocketAddr {
		match addr {
			SocketAddr::V6(v6) => match v6.ip().to_ipv4_mapped() {
				Some(v4) => SocketAddr::new(v4.into(), addr.port()),
				None => addr,
			},
			v4 => v4,
		}
	}
	canonical(a) == canonical(b)
}

/// Parse one `ADDRESS:PORT` cell of the kernel's TCP table.
///
/// The address is hex, in host byte order per 32-bit word: eight hex digits for
/// IPv4, thirty-two for IPv6.
fn parse_address(cell: &str) -> Option<SocketAddr> {
	let (addr, port) = cell.rsplit_once(':')?;
	let port = u16::from_str_radix(port, 16).ok()?;

	let words: Vec<u32> = addr
		.as_bytes()
		.chunks(8)
		.map(|chunk| u32::from_str_radix(std::str::from_utf8(chunk).ok()?, 16).ok())
		.collect::<Option<Vec<u32>>>()?;

	let ip = match words.len() {
		1 => std::net::IpAddr::from(words[0].to_le_bytes()),
		4 => {
			let mut bytes = [0u8; 16];
			for (i, word) in words.iter().enumerate() {
				bytes[i * 4..(i + 1) * 4].copy_from_slice(&word.to_le_bytes());
			}
			std::net::IpAddr::from(bytes)
		}
		_ => return None,
	};
	Some(SocketAddr::new(ip, port))
}

#[cfg(test)]
mod tests {
	use super::*;

	// Both ends of a connection appear: the caller's socket, owned by the user
	// it runs as, and the daemon's accepted socket, owned by root. Matching on
	// the caller's end is what distinguishes them — reading the daemon's own row
	// would have every caller look like root.
	//
	// 0x2047 = 8263 (the daemon), 0xC142 = 49474 (the caller).
	const TCP4: &str = "\
  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 0100007F:C142 0100007F:2047 01 00000000:00000000 00:00000000 00000000   997        0 51231 1 0000 20 0
   1: 0100007F:2047 0100007F:C142 01 00000000:00000000 00:00000000 00000000     0        0 51232 1 0000 20 0
   2: 0100007F:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 4113 1 0000 10 0
";

	const TCP6: &str = "\
  sl  local_address                         remote_address                        st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 00000000000000000000000001000000:C143 00000000000000000000000001000000:2047 01 00000000:00000000 00:00000000 00000000  1001        0 51999 1 0000 20 0
";

	fn addr(s: &str) -> SocketAddr {
		s.parse().unwrap()
	}

	#[test]
	fn a_connections_owner_is_read_from_the_kernels_table() {
		// The caller's socket is the mirror of ours: its local end is our peer,
		// and its owner is the user the caller runs as — not root, which owns
		// the end the daemon accepted on.
		let uid = find_owner(TCP4, addr("127.0.0.1:49474"), addr("127.0.0.1:8263"));
		assert_eq!(uid, Some(997));
	}

	#[test]
	fn a_connection_that_is_not_there_names_nobody() {
		assert_eq!(
			find_owner(TCP4, addr("127.0.0.1:49999"), addr("127.0.0.1:8263")),
			None
		);
		// A listening socket is not a connection, so its remote end matches
		// nothing we accepted.
		assert_eq!(
			find_owner(TCP4, addr("127.0.0.1:8080"), addr("127.0.0.1:8263")),
			None
		);
	}

	#[test]
	fn the_v6_table_is_read_too() {
		let uid = find_owner(TCP6, addr("[::1]:49475"), addr("[::1]:8263"));
		assert_eq!(uid, Some(1001));
	}

	#[test]
	fn a_v4_mapped_endpoint_matches_its_v4_form() {
		// A connection to 127.0.0.1 on a dual-stack listener appears mapped, and
		// must still match the address axum reports.
		assert!(same_endpoint(
			addr("[::ffff:127.0.0.1]:8263"),
			addr("127.0.0.1:8263")
		));
		assert!(!same_endpoint(
			addr("[::ffff:127.0.0.1]:8263"),
			addr("127.0.0.1:8264")
		));
	}

	#[test]
	fn the_superuser_is_always_permitted_and_one_further_user_may_be() {
		let none = Permitted::new(None);
		assert!(none.admits(0));
		assert!(!none.admits(997));

		let caddy = Permitted::new(Some(997));
		assert!(caddy.admits(0));
		assert!(caddy.admits(997));
		assert!(!caddy.admits(1001));
	}

	#[test]
	fn a_numeric_user_resolves_without_a_lookup() {
		assert_eq!(resolve_user("997").unwrap(), 997);
		assert_eq!(resolve_user(" 0 ").unwrap(), 0);
	}

	#[test]
	fn an_address_cell_parses_as_host_ordered_hex() {
		assert_eq!(parse_address("0100007F:1F90"), Some(addr("127.0.0.1:8080")));
		assert_eq!(
			parse_address("00000000000000000000000001000000:1F90"),
			Some(addr("[::1]:8080"))
		);
		assert_eq!(parse_address("nonsense"), None);
	}
}

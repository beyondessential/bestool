//! Establishing a device's own sticker secret: which source wins, whether the cache still holds, and
//! the derivation when it does not.
//!
//! Behaviour is specified in BLI-BID and in BLI-KEY, "Deriving on the device". The memory-hard
//! derivation is paid once and cached; establishing whether the cache still holds is a comparison of
//! cheap reads against the board, not a rederivation.

use std::{
	fs,
	path::{Path, PathBuf},
};

use bliti_core::{
	board_id::{
		BoardIdSource, CacheDecision, CacheState, OneTimeProgrammableSource, PlatformSerial,
		RaspberryPiSerialSource, SmbiosSystemUuidSource, SourceKind, evaluate_cache, select,
		strongest_present,
	},
	key_schedule::{
		STICKER_SECRET_LEN, StickerSecret, VERSION, check_memory, derive_sticker_secret,
	},
};
use serde::{Deserialize, Serialize};

/// Where the derived secret and the board it was derived from are cached. It is a cache the device
/// can rebuild, not authoritative state: losing it costs one derivation.
pub const DEFAULT_CACHE_PATH: &str = "/var/lib/bliti/identity.json";

/// The device's own identity, once established.
pub struct Identity {
	/// The sticker secret this board derives.
	pub secret: StickerSecret,
	/// Which kind of source it was derived from.
	pub kind: SourceKind,
	/// Whether the memory-hard derivation had to run, rather than the cache standing.
	pub derived: bool,
}

/// The cache file's contents.
#[derive(Debug, Serialize, Deserialize)]
struct CacheFile {
	/// The key-schedule version the secret was derived under. A device that finds a different one has
	/// been upgraded across a version change and rederives.
	version: u8,
	/// Which kind of source won the precedence.
	kind: String,
	/// The platform serial of the board, hex-encoded, or absent where the board offers none.
	platform_serial: Option<String>,
	/// The derived secret, hex-encoded.
	secret: String,
}

/// Assemble the board-ID backends this build can see.
///
/// Which backends are registered decides which source wins the precedence, and so which secret the
/// board derives, which is why [`guard_unreadable_sources`] exists.
pub fn sources() -> Vec<Box<dyn BoardIdSource>> {
	#[cfg_attr(
		not(feature = "tpm"),
		expect(
			unused_mut,
			reason = "the TPM source is pushed only when that feature is on"
		)
	)]
	let mut sources: Vec<Box<dyn BoardIdSource>> = vec![
		Box::new(OneTimeProgrammableSource::new()),
		Box::new(RaspberryPiSerialSource::new()),
		Box::new(SmbiosSystemUuidSource::new()),
	];
	#[cfg(feature = "tpm")]
	sources.push(Box::new(
		bliti_core::board_id::TpmEndorsementKeySource::new(),
	));
	sources
}

/// Refuse to derive on a board carrying a source this build cannot read.
///
/// A build without the `tpm` feature cannot see a TPM, so on a board that has one it would derive
/// from the serial number instead and produce a secret that does not match the sticker on the
/// enclosure. That is worse than not starting, because the device would advertise a handle nobody
/// can match while looking healthy, so it is refused here.
pub fn guard_unreadable_sources() -> Result<(), IdentityError> {
	#[cfg(not(feature = "tpm"))]
	for node in ["/dev/tpmrm0", "/dev/tpm0"] {
		if Path::new(node).exists() {
			return Err(IdentityError::UnreadableSource {
				kind: "TPM",
				detail: format!(
					"{node} is present, but this build was made without TPM support, so it would \
					 derive from a weaker source and not match this board's sticker"
				),
			});
		}
	}
	Ok(())
}

/// Read the board's platform serial, which identifies it across a change of winning source. Cheap,
/// and read on every start.
pub fn platform_serial(
	sources: &[Box<dyn BoardIdSource>],
) -> Result<PlatformSerial, IdentityError> {
	for source in sources {
		if source.kind().is_platform_serial()
			&& source.probe().map_err(IdentityError::BoardId)?
				== bliti_core::board_id::Presence::Present
		{
			return Ok(Some(source.read().map_err(IdentityError::BoardId)?));
		}
	}
	Ok(None)
}

/// Establish the device's sticker secret, deriving only where the cache does not hold.
pub fn establish(cache_path: &Path) -> Result<Identity, IdentityError> {
	guard_unreadable_sources()?;

	let sources = sources();
	let refs: Vec<&dyn BoardIdSource> = sources.iter().map(AsRef::as_ref).collect();

	// Both of these are cheap: a file read and a set of presence probes, with no source value read
	// and no key generation inside a TPM.
	let serial = platform_serial(&sources)?;
	let strongest = strongest_present(&refs).map_err(IdentityError::BoardId)?;
	let cached = read_cache(cache_path)?;

	match evaluate_cache(cached.as_ref().map(|(state, _)| state), &serial, strongest) {
		CacheDecision::Fresh => {
			let (state, secret) = cached.expect("a fresh cache was read");
			Ok(Identity {
				secret,
				kind: state.board_id_kind,
				derived: false,
			})
		}
		CacheDecision::StickerDead => Err(IdentityError::StickerDead {
			cached: cached.map(|(state, _)| state.board_id_kind),
			found: strongest,
		}),
		CacheDecision::Rederive => {
			let board_id = select(&refs).map_err(IdentityError::BoardId)?;

			// The derivation needs its full memory parameter at once and is killed by the operating
			// system rather than told the allocation failed, so establish there is room first.
			check_memory(available_memory_bytes()).map_err(IdentityError::Key)?;

			let secret = derive_sticker_secret(&board_id).map_err(IdentityError::Key)?;
			write_cache(
				cache_path,
				&CacheState {
					board_id_kind: board_id.kind(),
					platform_serial: serial,
				},
				&secret,
			)?;
			Ok(Identity {
				secret,
				kind: board_id.kind(),
				derived: true,
			})
		}
	}
}

/// Bytes of memory available, from the kernel's own estimate of what can be allocated without
/// swapping. `MemAvailable` is the right figure rather than `MemFree`, which ignores reclaimable
/// cache.
fn available_memory_bytes() -> u64 {
	fs::read_to_string("/proc/meminfo")
		.ok()
		.and_then(|meminfo| {
			meminfo.lines().find_map(|line| {
				let rest = line.strip_prefix("MemAvailable:")?;
				let kib: u64 = rest.split_whitespace().next()?.parse().ok()?;
				Some(kib * 1024)
			})
		})
		.unwrap_or(u64::MAX)
}

fn kind_name(kind: SourceKind) -> &'static str {
	match kind {
		SourceKind::TpmEndorsementKey => "tpm-endorsement-key",
		SourceKind::OneTimeProgrammable => "one-time-programmable",
		SourceKind::RaspberryPiSerial => "raspberry-pi-serial",
		SourceKind::SmbiosSystemUuid => "smbios-system-uuid",
	}
}

fn kind_from_name(name: &str) -> Option<SourceKind> {
	SourceKind::ALL.into_iter().find(|k| kind_name(*k) == name)
}

/// Read the cache, or `None` where it is absent or does not apply. A cache that cannot be understood
/// is treated as absent: it costs one derivation to rebuild, which is better than refusing to start.
fn read_cache(path: &Path) -> Result<Option<(CacheState, StickerSecret)>, IdentityError> {
	let raw = match fs::read_to_string(path) {
		Ok(raw) => raw,
		Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
		Err(err) => return Err(IdentityError::Cache(err.to_string())),
	};
	let Ok(file) = serde_json::from_str::<CacheFile>(&raw) else {
		tracing::warn!("identity cache is unreadable; rederiving");
		return Ok(None);
	};
	if file.version != VERSION {
		tracing::warn!(
			cached = file.version,
			current = VERSION,
			"identity cache is from another key-schedule version; rederiving"
		);
		return Ok(None);
	}
	let (Some(kind), Ok(secret)) = (kind_from_name(&file.kind), hex::decode(&file.secret)) else {
		tracing::warn!("identity cache is unreadable; rederiving");
		return Ok(None);
	};
	let Ok(secret): Result<[u8; STICKER_SECRET_LEN], _> = secret.try_into() else {
		tracing::warn!("identity cache holds a secret of the wrong length; rederiving");
		return Ok(None);
	};
	let platform_serial = match file.platform_serial.as_deref().map(hex::decode) {
		Some(Ok(serial)) => Some(serial),
		Some(Err(_)) => return Ok(None),
		None => None,
	};
	Ok(Some((
		CacheState {
			board_id_kind: kind,
			platform_serial,
		},
		StickerSecret::from_bytes(secret),
	)))
}

fn write_cache(
	path: &Path,
	state: &CacheState,
	secret: &StickerSecret,
) -> Result<(), IdentityError> {
	if let Some(parent) = path.parent() {
		fs::create_dir_all(parent).map_err(|err| IdentityError::Cache(err.to_string()))?;
	}
	let file = CacheFile {
		version: VERSION,
		kind: kind_name(state.board_id_kind).to_owned(),
		platform_serial: state.platform_serial.as_ref().map(hex::encode),
		secret: hex::encode(secret.as_bytes()),
	};
	let body =
		serde_json::to_string_pretty(&file).map_err(|err| IdentityError::Cache(err.to_string()))?;

	// Write through a temporary file in the same directory, so a start interrupted part-way leaves
	// either the old cache or the new one rather than a truncated file.
	let temporary = path.with_extension("json.new");
	fs::write(&temporary, body).map_err(|err| IdentityError::Cache(err.to_string()))?;
	restrict(&temporary)?;
	fs::rename(&temporary, path).map_err(|err| IdentityError::Cache(err.to_string()))
}

/// The cache holds the sticker secret, which is the credential, so it is readable only by the user
/// the daemon runs as.
fn restrict(path: &Path) -> Result<(), IdentityError> {
	#[cfg(unix)]
	{
		use std::os::unix::fs::PermissionsExt;
		fs::set_permissions(path, fs::Permissions::from_mode(0o600))
			.map_err(|err| IdentityError::Cache(err.to_string()))?;
	}
	Ok(())
}

/// The default cache path, overridable for testing and for running unprivileged.
pub fn default_cache_path() -> PathBuf {
	PathBuf::from(DEFAULT_CACHE_PATH)
}

/// A failure establishing the device's identity. These leave the device unreachable over the channel,
/// so they are reported where the device is rather than to a client (BLI, "Reporting").
#[derive(Debug, thiserror::Error)]
pub enum IdentityError {
	/// No usable board ID source, or a backend failed.
	#[error(transparent)]
	BoardId(bliti_core::board_id::BoardIdError),

	/// The derivation could not run.
	#[error(transparent)]
	Key(bliti_core::key_schedule::KeyError),

	/// The board has gained hardware carrying a stronger source, so the sticker on its enclosure is
	/// dead and no client can reach it. Recovering means printing a new sticker for this board.
	#[error(
		"this board's sticker is dead: it was derived from {cached:?} but the board now offers \
		 {found:?}, so the printed sticker no longer matches it. Print a new sticker for this board."
	)]
	StickerDead {
		/// The kind of source the cached secret was derived from.
		cached: Option<SourceKind>,
		/// The strongest kind of source the board offers now.
		found: Option<SourceKind>,
	},

	/// This build cannot read a source the board carries, so deriving would give the wrong secret.
	#[cfg_attr(
		feature = "tpm",
		expect(dead_code, reason = "only raised by a build without TPM support")
	)]
	#[error("refusing to derive: {detail}")]
	UnreadableSource {
		/// The kind of source that cannot be read.
		kind: &'static str,
		/// What was found and why it is refused.
		detail: String,
	},

	/// The cache could not be read or written.
	#[error("identity cache: {0}")]
	Cache(String),
}

//! The board ID: the firmware-provided identifier every other value in bliti descends from.
//!
//! Behaviour is specified in `.workhorse/specs/bliti/board-id.md` (BLI-BID). The board ID makes
//! the sticker secret reproducible from the board alone: the same sticker regenerates from the
//! board with no per-device database to keep in sync. It is not a secret; what protects it is the
//! cost of the derivation in [`crate::key_schedule`] and the size of its space.

use std::fmt;

#[cfg(feature = "backends")]
mod backends;
#[cfg(feature = "tpm")]
pub use backends::TpmEndorsementKeySource;
#[cfg(feature = "backends")]
pub use backends::{OneTimeProgrammableSource, RaspberryPiSerialSource, SmbiosSystemUuidSource};

/// The kind of source a board ID was read from.
///
/// Each kind carries a **tag byte**, mixed into the derivation in [`crate::key_schedule`] so that a
/// value that is byte-identical across two kinds of source still derives a different secret. The tag
/// is part of the versioned key schedule (BLI-KEY): changing it re-derives every board ID taken
/// under the old value, so the assignments here are load-bearing and never reused.
///
/// The kinds are also ordered by **precedence**, strongest first, evaluated by kind rather than by
/// platform so a board gains a stronger source simply by having the hardware for it. See
/// [`SourceKind::is_stronger_than`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SourceKind {
	/// The name of the TPM 2.0 Endorsement Key: the strongest source, with a space large enough
	/// that no derivation cost is load-bearing.
	TpmEndorsementKey,
	/// Customer-programmable one-time-programmable memory that has been written.
	OneTimeProgrammable,
	/// The Raspberry Pi device-tree serial. One of the two platform-serial kinds.
	RaspberryPiSerial,
	/// The SMBIOS system UUID on a UEFI machine. One of the two platform-serial kinds.
	SmbiosSystemUuid,
}

impl SourceKind {
	/// Every kind, strongest first. This order is the precedence.
	pub const ALL: [SourceKind; 4] = [
		SourceKind::TpmEndorsementKey,
		SourceKind::OneTimeProgrammable,
		SourceKind::RaspberryPiSerial,
		SourceKind::SmbiosSystemUuid,
	];

	/// The tag byte mixed into the derivation. Stable and versioned; never reused. Zero is reserved
	/// as "no source" and is not assigned to any kind.
	pub const fn tag(self) -> u8 {
		match self {
			SourceKind::TpmEndorsementKey => 1,
			SourceKind::OneTimeProgrammable => 2,
			SourceKind::RaspberryPiSerial => 3,
			SourceKind::SmbiosSystemUuid => 4,
		}
	}

	/// Precedence rank, higher being stronger. Used only to compare kinds; the absolute values
	/// carry no other meaning and are not part of the wire format.
	const fn rank(self) -> u8 {
		match self {
			SourceKind::TpmEndorsementKey => 3,
			SourceKind::OneTimeProgrammable => 2,
			// The two platform-serial kinds share a conceptual tier and never co-occur on one
			// board, so the order between them is a formality that only makes selection total.
			SourceKind::RaspberryPiSerial => 1,
			SourceKind::SmbiosSystemUuid => 0,
		}
	}

	/// Whether this kind is a platform serial number: the last tier of the precedence, present on
	/// every board in scope and stable when stronger hardware is fitted, so it identifies a board
	/// across a change of source (BLI-BID, "When the board ID changes").
	pub const fn is_platform_serial(self) -> bool {
		matches!(
			self,
			SourceKind::RaspberryPiSerial | SourceKind::SmbiosSystemUuid
		)
	}

	/// Whether this kind wins the precedence over `other`.
	pub const fn is_stronger_than(self, other: SourceKind) -> bool {
		self.rank() > other.rank()
	}
}

impl fmt::Display for SourceKind {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		let name = match self {
			SourceKind::TpmEndorsementKey => "TPM Endorsement Key",
			SourceKind::OneTimeProgrammable => "one-time-programmable memory",
			SourceKind::RaspberryPiSerial => "Raspberry Pi serial",
			SourceKind::SmbiosSystemUuid => "SMBIOS system UUID",
		};
		f.write_str(name)
	}
}

/// A board ID: the raw bytes of a source value together with the kind of source they came from.
///
/// The bytes are the raw value, most significant first, never a text rendering of it: a Raspberry Pi
/// serial is the eight bytes it denotes, not its characters, and an SMBIOS system UUID is the
/// sixteen bytes it denotes, not its dashed string (BLI-KEY, "What is derived from").
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BoardId {
	kind: SourceKind,
	bytes: Vec<u8>,
}

impl BoardId {
	/// Construct a board ID from a source kind and its raw bytes.
	///
	/// Fails if the bytes are empty, or if they are a placeholder that carries no identity (all
	/// zeros or all ones), because deriving from a placeholder would give every board in the same
	/// position the same secret (BLI-BID, "Sources that carry no identity").
	pub fn new(kind: SourceKind, bytes: impl Into<Vec<u8>>) -> Result<Self, BoardIdError> {
		let bytes = bytes.into();
		if bytes.is_empty() {
			return Err(BoardIdError::Empty(kind));
		}
		if is_sentinel(&bytes) {
			return Err(BoardIdError::Placeholder(kind));
		}
		Ok(Self { kind, bytes })
	}

	/// The kind of source this board ID came from.
	pub fn kind(&self) -> SourceKind {
		self.kind
	}

	/// The raw bytes of the source value, most significant first.
	pub fn raw(&self) -> &[u8] {
		&self.bytes
	}
}

/// Whether a value is a placeholder that carries no identity: all zeros or all ones, whatever its
/// nominal width. A vendor-specific constant is not detectable generically and is rejected by the
/// backend that knows it (in its [`BoardIdSource::probe`]), not here.
pub fn is_sentinel(bytes: &[u8]) -> bool {
	!bytes.is_empty() && (bytes.iter().all(|&b| b == 0x00) || bytes.iter().all(|&b| b == 0xff))
}

/// Whether a source is present on a board, established cheaply and without reading its value.
///
/// Presence is separate from reading (BLI-BID, "Probing and reading"): a TPM Endorsement Key name
/// costs a key generation inside the TPM to read, so the precedence is evaluated by probing every
/// source for presence and reading a value only from the one that wins.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Presence {
	/// The source is not present on this board.
	Absent,
	/// The source is present and holds an identity.
	Present,
	/// The source is present but holds a placeholder — unwritten one-time-programmable memory, or a
	/// vendor constant. Precedence falls through it to the next source.
	Placeholder,
}

/// A backend that reads one kind of board ID source.
///
/// Presence is cheap to establish and the value is not; implementations keep [`probe`] free of any
/// expensive read (no TPM key generation), and do the costly work only in [`read`], which is called
/// only on the source that wins the precedence.
///
/// [`probe`]: BoardIdSource::probe
/// [`read`]: BoardIdSource::read
pub trait BoardIdSource {
	/// The kind of source this backend reads.
	fn kind(&self) -> SourceKind;

	/// Establish whether this source is present, cheaply and without reading its value.
	fn probe(&self) -> Result<Presence, BoardIdError>;

	/// Read the source value, as raw bytes most significant first. Called only on the winning
	/// source. May be expensive.
	fn read(&self) -> Result<Vec<u8>, BoardIdError>;
}

/// Evaluate the precedence over a set of backends and read the winning board ID.
///
/// Backends are probed strongest first; the first one present with an identity wins, and only its
/// value is read (BLI-BID, "Probing and reading"). Reaching the end with no usable source is a
/// failure rather than something to derive past.
pub fn select(sources: &[&dyn BoardIdSource]) -> Result<BoardId, BoardIdError> {
	for kind in SourceKind::ALL {
		let Some(source) = sources.iter().find(|s| s.kind() == kind) else {
			continue;
		};
		match source.probe()? {
			Presence::Present => {
				let bytes = source.read()?;
				return BoardId::new(kind, bytes);
			}
			Presence::Placeholder | Presence::Absent => continue,
		}
	}
	Err(BoardIdError::NoUsableSource)
}

/// Probe the set of backends and report the strongest kind present with an identity, if any. Reads
/// no value, so it is cheap even where a TPM is present. Used by the cache check in
/// [`crate::key_schedule`] to decide whether a rederivation is needed.
pub fn strongest_present(
	sources: &[&dyn BoardIdSource],
) -> Result<Option<SourceKind>, BoardIdError> {
	for kind in SourceKind::ALL {
		let Some(source) = sources.iter().find(|s| s.kind() == kind) else {
			continue;
		};
		if source.probe()? == Presence::Present {
			return Ok(Some(kind));
		}
	}
	Ok(None)
}

/// A board ID source that yields a value fixed at construction, for exercising the chain against
/// known inputs without hardware (test-cases, "A board ID override is available for tests").
#[derive(Debug, Clone)]
pub struct TestSource {
	kind: SourceKind,
	presence: Presence,
	value: Vec<u8>,
}

impl TestSource {
	/// A source present with the given value.
	pub fn present(kind: SourceKind, value: impl Into<Vec<u8>>) -> Self {
		Self {
			kind,
			presence: Presence::Present,
			value: value.into(),
		}
	}

	/// A source that is absent.
	pub fn absent(kind: SourceKind) -> Self {
		Self {
			kind,
			presence: Presence::Absent,
			value: Vec::new(),
		}
	}

	/// A source present but holding a placeholder, which precedence falls through.
	pub fn placeholder(kind: SourceKind, value: impl Into<Vec<u8>>) -> Self {
		Self {
			kind,
			presence: Presence::Placeholder,
			value: value.into(),
		}
	}
}

impl BoardIdSource for TestSource {
	fn kind(&self) -> SourceKind {
		self.kind
	}

	fn probe(&self) -> Result<Presence, BoardIdError> {
		Ok(self.presence)
	}

	fn read(&self) -> Result<Vec<u8>, BoardIdError> {
		Ok(self.value.clone())
	}
}

/// The platform serial of a board: the last-tier source that identifies it across a change of the
/// winning source. Read cheaply on every start. `None` where the board offers no platform serial.
pub type PlatformSerial = Option<Vec<u8>>;

/// What a device knows about itself from the last derivation, held so the memory-hard derivation
/// runs only when the board no longer matches it (BLI-KEY, "Deriving on the device"). It is a cache
/// the device can rebuild, not authoritative state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheState {
	/// The kind of source that won the precedence when the secret was derived.
	pub board_id_kind: SourceKind,
	/// The platform serial of the board the secret was derived on.
	pub platform_serial: PlatformSerial,
}

/// The decision reached by comparing a device's cache against the board in front of it, using only
/// the cheap reads (the platform serial and which kinds of source are present).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheDecision {
	/// The cache still holds: the cached secret stands and no source value is read and no
	/// derivation runs.
	Fresh,
	/// The board has gained hardware carrying a stronger source, or otherwise no longer derives to
	/// the cached secret, and the sticker on its enclosure is dead. Reported rather than derived
	/// past.
	StickerDead,
	/// The board does not match the cache and is not a dead sticker: the cache is absent, or the
	/// board is a different one (its platform serial differs, as when a disk is moved into another
	/// enclosure). The precedence is evaluated, the winning source read, and the secret derived.
	Rederive,
}

/// Decide, from the cheap reads alone, whether a cached secret still holds.
///
/// `cache` is what the device last recorded, or `None` if it has none. `observed_serial` is the
/// platform serial read from the board now. `strongest_present` is the strongest kind of source
/// found present now, from [`strongest_present`], or `None` if none is present.
///
/// Implements the start-up decision of BLI-KEY, "Deriving on the device", and the identity-change
/// rules of BLI-BID, "When the board ID changes".
pub fn evaluate_cache(
	cache: Option<&CacheState>,
	observed_serial: &PlatformSerial,
	strongest_present: Option<SourceKind>,
) -> CacheDecision {
	let Some(cache) = cache else {
		return CacheDecision::Rederive;
	};

	// No source present at all is not a cache question; it is the no-usable-source failure, surfaced
	// when the precedence is evaluated. Send it down the rederive path to be reported there.
	let Some(present) = strongest_present else {
		return CacheDecision::Rederive;
	};

	if observed_serial.is_some() {
		if *observed_serial != cache.platform_serial {
			// A different board: the disk sits in another enclosure now. It derives from the board
			// it is on and matches the sticker already fixed to that enclosure. Not a fault.
			return CacheDecision::Rederive;
		}
		// Same board. The cache holds only if the strongest source present is still the one it
		// derived from; any other outcome orphans the sticker.
		if present == cache.board_id_kind {
			CacheDecision::Fresh
		} else {
			CacheDecision::StickerDead
		}
	} else {
		// A board that offers no platform serial has no weaker source for a stronger one to
		// supersede, so any change in which kind wins is reported. The strongest kind present is the
		// only thing comparable cheaply.
		if present == cache.board_id_kind {
			CacheDecision::Fresh
		} else {
			CacheDecision::StickerDead
		}
	}
}

/// A failure reading or selecting a board ID.
#[derive(Debug, thiserror::Error)]
pub enum BoardIdError {
	/// No source in the precedence held a usable identity.
	#[error("no usable board ID source: every source is absent or holds a placeholder")]
	NoUsableSource,

	/// A source produced an empty value.
	#[error("board ID source {0} produced an empty value")]
	Empty(SourceKind),

	/// A source produced a placeholder value that carries no identity.
	#[error("board ID source {0} holds a placeholder value carrying no identity")]
	Placeholder(SourceKind),

	/// A backend failed to read its source.
	#[error("reading board ID source {kind}: {message}")]
	Backend {
		/// The source being read.
		kind: SourceKind,
		/// What went wrong.
		message: String,
	},
}

#[cfg(test)]
mod tests {
	use super::*;

	/// A source whose `read` panics, to prove a code path only reads the value it is entitled to.
	struct ReadPanics {
		kind: SourceKind,
		presence: Presence,
	}

	impl BoardIdSource for ReadPanics {
		fn kind(&self) -> SourceKind {
			self.kind
		}
		fn probe(&self) -> Result<Presence, BoardIdError> {
			Ok(self.presence)
		}
		fn read(&self) -> Result<Vec<u8>, BoardIdError> {
			panic!("read must not be called on {}", self.kind);
		}
	}

	#[test]
	fn tags_are_stable_and_distinct() {
		// Load-bearing: these tags are versioned into every sticker.
		assert_eq!(SourceKind::TpmEndorsementKey.tag(), 1);
		assert_eq!(SourceKind::OneTimeProgrammable.tag(), 2);
		assert_eq!(SourceKind::RaspberryPiSerial.tag(), 3);
		assert_eq!(SourceKind::SmbiosSystemUuid.tag(), 4);
	}

	#[test]
	fn precedence_is_strongest_first() {
		assert!(SourceKind::TpmEndorsementKey.is_stronger_than(SourceKind::OneTimeProgrammable));
		assert!(SourceKind::OneTimeProgrammable.is_stronger_than(SourceKind::RaspberryPiSerial));
		assert!(SourceKind::RaspberryPiSerial.is_stronger_than(SourceKind::SmbiosSystemUuid));
		assert!(!SourceKind::SmbiosSystemUuid.is_stronger_than(SourceKind::TpmEndorsementKey));
	}

	#[test]
	fn select_picks_strongest_present() {
		let tpm = TestSource::present(SourceKind::TpmEndorsementKey, vec![1, 2, 3, 4]);
		let serial = TestSource::present(SourceKind::RaspberryPiSerial, vec![9; 8]);
		let selected = select(&[&serial, &tpm]).unwrap();
		assert_eq!(selected.kind(), SourceKind::TpmEndorsementKey);
		assert_eq!(selected.raw(), &[1, 2, 3, 4]);
	}

	#[test]
	fn select_reads_only_the_winner() {
		// A stronger source is present; a weaker one whose read panics must never be read.
		let tpm = TestSource::present(SourceKind::TpmEndorsementKey, vec![7, 7]);
		let serial = ReadPanics {
			kind: SourceKind::RaspberryPiSerial,
			presence: Presence::Present,
		};
		let selected = select(&[&serial, &tpm]).unwrap();
		assert_eq!(selected.kind(), SourceKind::TpmEndorsementKey);
	}

	#[test]
	fn placeholder_falls_through_to_next_source() {
		// Unwritten OTP reads as a placeholder; precedence falls through to the serial number.
		let otp = TestSource::placeholder(SourceKind::OneTimeProgrammable, vec![0; 32]);
		let serial =
			TestSource::present(SourceKind::RaspberryPiSerial, vec![0xf3, 0x75, 0x65, 0x10]);
		let selected = select(&[&otp, &serial]).unwrap();
		assert_eq!(selected.kind(), SourceKind::RaspberryPiSerial);
	}

	#[test]
	fn no_usable_source_is_a_failure() {
		let otp = TestSource::placeholder(SourceKind::OneTimeProgrammable, vec![0; 32]);
		let serial = TestSource::absent(SourceKind::RaspberryPiSerial);
		assert!(matches!(
			select(&[&otp, &serial]),
			Err(BoardIdError::NoUsableSource)
		));
	}

	#[test]
	fn board_id_rejects_placeholders() {
		assert!(matches!(
			BoardId::new(SourceKind::RaspberryPiSerial, vec![0u8; 8]),
			Err(BoardIdError::Placeholder(_))
		));
		assert!(matches!(
			BoardId::new(SourceKind::RaspberryPiSerial, vec![0xffu8; 8]),
			Err(BoardIdError::Placeholder(_))
		));
		assert!(matches!(
			BoardId::new(SourceKind::RaspberryPiSerial, Vec::new()),
			Err(BoardIdError::Empty(_))
		));
	}

	#[test]
	fn strongest_present_reads_no_value() {
		// Probing for presence must not read a value: a TPM is probed without a key generation.
		let tpm = ReadPanics {
			kind: SourceKind::TpmEndorsementKey,
			presence: Presence::Present,
		};
		let serial = ReadPanics {
			kind: SourceKind::RaspberryPiSerial,
			presence: Presence::Present,
		};
		let strongest = strongest_present(&[&serial, &tpm]).unwrap();
		assert_eq!(strongest, Some(SourceKind::TpmEndorsementKey));
	}

	fn cache(kind: SourceKind, serial: Option<Vec<u8>>) -> CacheState {
		CacheState {
			board_id_kind: kind,
			platform_serial: serial,
		}
	}

	#[test]
	fn cache_fresh_when_serial_and_kind_match() {
		let c = cache(SourceKind::RaspberryPiSerial, Some(vec![1, 2, 3]));
		let decision = evaluate_cache(
			Some(&c),
			&Some(vec![1, 2, 3]),
			Some(SourceKind::RaspberryPiSerial),
		);
		assert_eq!(decision, CacheDecision::Fresh);
	}

	#[test]
	fn cache_dead_when_board_gains_stronger_hardware() {
		// Same board (serial unchanged), but a TPM is now present: the sticker is dead.
		let c = cache(SourceKind::RaspberryPiSerial, Some(vec![1, 2, 3]));
		let decision = evaluate_cache(
			Some(&c),
			&Some(vec![1, 2, 3]),
			Some(SourceKind::TpmEndorsementKey),
		);
		assert_eq!(decision, CacheDecision::StickerDead);
	}

	#[test]
	fn cache_rederive_when_disk_moved_to_another_board() {
		// A different platform serial is a different board: derive and match its own sticker.
		let c = cache(SourceKind::RaspberryPiSerial, Some(vec![1, 2, 3]));
		let decision = evaluate_cache(
			Some(&c),
			&Some(vec![4, 5, 6]),
			Some(SourceKind::RaspberryPiSerial),
		);
		assert_eq!(decision, CacheDecision::Rederive);
	}

	#[test]
	fn cache_rederive_when_absent() {
		let decision = evaluate_cache(
			None,
			&Some(vec![1, 2, 3]),
			Some(SourceKind::RaspberryPiSerial),
		);
		assert_eq!(decision, CacheDecision::Rederive);
	}

	#[test]
	fn cache_dead_when_no_serial_and_kind_changes() {
		// A board offering no platform serial reports any change in its board ID.
		let c = cache(SourceKind::TpmEndorsementKey, None);
		let decision = evaluate_cache(Some(&c), &None, Some(SourceKind::OneTimeProgrammable));
		assert_eq!(decision, CacheDecision::StickerDead);
	}
}

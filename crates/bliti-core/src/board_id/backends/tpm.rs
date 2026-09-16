//! The TPM 2.0 Endorsement Key as a board ID source: the strongest source in the precedence.
//!
//! Specified in BLI-BID, "TPM Endorsement Key". The board ID is the *name* of the Endorsement Key:
//! the hash algorithm identifier followed by the digest of the key's public area, which is what the
//! TPM itself computes and what the specification means by a key's name. On a SHA-256 TPM that is 34
//! bytes: two bytes of algorithm identifier and a 32-byte digest.
//!
//! The key is regenerated from the endorsement seed under the pinned template rather than read from
//! wherever provisioning software may have persisted it, because a persisted copy is not guaranteed
//! to exist on a freshly imaged machine while the seed and the template always are.
//!
//! The template is pinned to the TCG low-range RSA 2048 Endorsement Key. A TPM holds one Endorsement
//! Key per algorithm, so the algorithm is part of the derivation: changing it re-derives every board
//! ID taken under the old one, which is why it is versioned along with everything else in
//! [`crate::key_schedule`].

use std::path::{Path, PathBuf};

use tss_esapi::{
	Context,
	abstraction::{AsymmetricAlgorithmSelection, ek},
	handles::KeyHandle,
	interface_types::key_bits::RsaKeyBits,
	tcti_ldr::{DeviceConfig, TctiNameConf},
};

use crate::board_id::{BoardIdError, BoardIdSource, Presence, SourceKind};

/// The resource manager device, preferred because it multiplexes access with anything else on the
/// system that speaks to the TPM.
pub const DEFAULT_TPM_DEVICE: &str = "/dev/tpmrm0";

/// The raw TPM device, used where no resource manager is present.
pub const FALLBACK_TPM_DEVICE: &str = "/dev/tpm0";

/// The pinned Endorsement Key template: TCG low-range RSA 2048. Part of the versioned key schedule.
const EK_ALGORITHM: AsymmetricAlgorithmSelection =
	AsymmetricAlgorithmSelection::Rsa(RsaKeyBits::Rsa2048);

/// The TPM 2.0 Endorsement Key, reached through the TPM software stack.
#[derive(Debug, Clone)]
pub struct TpmEndorsementKeySource {
	device: Option<PathBuf>,
}

impl Default for TpmEndorsementKeySource {
	fn default() -> Self {
		Self::new()
	}
}

impl TpmEndorsementKeySource {
	/// Use whichever standard device node is present, preferring the resource manager.
	pub fn new() -> Self {
		Self { device: None }
	}

	/// Use a given device node, or any TCTI the stack understands when pointed at a simulator.
	pub fn at(device: impl Into<PathBuf>) -> Self {
		Self {
			device: Some(device.into()),
		}
	}

	/// The device node to talk to: the one configured, else the resource manager, else the raw
	/// device. `None` when the board carries no TPM, so presence is decided the same way whether the
	/// node was configured or found.
	fn device(&self) -> Option<PathBuf> {
		match &self.device {
			Some(device) => device.exists().then(|| device.clone()),
			None => [DEFAULT_TPM_DEVICE, FALLBACK_TPM_DEVICE]
				.into_iter()
				.map(Path::new)
				.find(|path| path.exists())
				.map(Path::to_path_buf),
		}
	}

	fn backend_message(&self, message: impl Into<String>) -> BoardIdError {
		BoardIdError::Backend {
			kind: SourceKind::TpmEndorsementKey,
			message: message.into(),
		}
	}

	/// Open a context against the device.
	fn context(&self) -> Result<Context, BoardIdError> {
		let device = self
			.device()
			.ok_or_else(|| self.backend_message("no TPM device node present"))?;
		let config: DeviceConfig = device
			.to_string_lossy()
			.parse()
			.map_err(|err| self.backend_message(format!("bad TPM device {device:?}: {err}")))?;
		Context::new(TctiNameConf::Device(config))
			.map_err(|err| self.backend_message(format!("opening TPM context: {err}")))
	}
}

impl BoardIdSource for TpmEndorsementKeySource {
	fn kind(&self) -> SourceKind {
		SourceKind::TpmEndorsementKey
	}

	/// Presence is the device node existing. Deliberately no key generation and no TPM command: a
	/// board carrying a TPM is probed without paying for the Endorsement Key (BLI-BID, "Probing and
	/// reading").
	fn probe(&self) -> Result<Presence, BoardIdError> {
		Ok(match self.device() {
			Some(_) => Presence::Present,
			None => Presence::Absent,
		})
	}

	/// Regenerate the Endorsement Key from the seed under the pinned template and return its name.
	/// This is a key generation inside the TPM, paid only by the source that wins the precedence.
	fn read(&self) -> Result<Vec<u8>, BoardIdError> {
		let mut context = self.context()?;
		let handle: KeyHandle = ek::create_ek_object_2(&mut context, EK_ALGORITHM, None)
			.map_err(|err| self.backend_message(format!("creating Endorsement Key: {err}")))?;

		let name = context
			.read_public(handle)
			.map(|(_public, name, _qualified)| name)
			.map_err(|err| self.backend_message(format!("reading Endorsement Key name: {err}")));

		// The key is transient; release it whether or not the read succeeded, so repeated starts do
		// not exhaust the TPM's object slots.
		let _ = context.flush_context(handle.into());

		let name = name?;
		let value = name.value().to_vec();
		if value.is_empty() {
			return Err(self.backend_message("TPM returned an empty Endorsement Key name"));
		}
		Ok(value)
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn absent_where_no_device_node_exists() {
		let source = TpmEndorsementKeySource::at("/nonexistent/bliti/tpmrm0");
		assert_eq!(source.probe().unwrap(), Presence::Absent);
	}

	#[test]
	fn present_where_a_device_node_exists() {
		// Any existing path stands in for a device node: presence is the node being there.
		let source = TpmEndorsementKeySource::at("/dev/null");
		assert_eq!(source.probe().unwrap(), Presence::Present);
	}

	#[test]
	fn probe_runs_no_tpm_command() {
		// Probing must not open a context or generate a key: it is a path check only, so it is safe
		// and instant even on a machine whose TPM is busy or permission-denied.
		let source = TpmEndorsementKeySource::new();
		let before = std::time::Instant::now();
		let _ = source.probe().unwrap();
		assert!(before.elapsed() < std::time::Duration::from_millis(50));
	}

	/// Reads the real TPM on this machine. Ignored by default because it needs a TPM and the
	/// privilege to reach it; run with `--ignored` on a machine that has both.
	#[test]
	#[ignore = "requires a TPM and access to its device node"]
	fn reads_a_reproducible_endorsement_key_name() {
		let source = TpmEndorsementKeySource::new();
		assert_eq!(source.probe().unwrap(), Presence::Present);

		let first = source.read().expect("read EK name");
		// A name is the algorithm identifier followed by the digest: 34 bytes under SHA-256.
		assert_eq!(&first[..2], &[0x00, 0x0b], "expected a SHA-256 name prefix");
		assert_eq!(first.len(), 34);

		// The whole scheme rests on this being reproducible from the seed and the template.
		let second = source.read().expect("read EK name again");
		assert_eq!(first, second);

		// Where the operator supplies the name this machine's TPM should produce, check it exactly.
		// This is how the backend is pinned against an independent implementation, without baking one
		// machine's Endorsement Key into the repository:
		//
		//     tpm2_createek -c ek.ctx -G rsa && tpm2_readpublic -c ek.ctx -n ek.name
		//     BLITI_EXPECT_EK_NAME=$(od -An -tx1 ek.name | tr -d ' \n')
		if let Ok(expected) = std::env::var("BLITI_EXPECT_EK_NAME") {
			let actual: String = first.iter().map(|b| format!("{b:02x}")).collect();
			assert_eq!(actual, expected.trim().to_lowercase());
		}
	}
}

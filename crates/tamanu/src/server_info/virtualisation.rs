//! Which hypervisor, if any, this host runs under.
//!
//! One vocabulary across platforms: the identifiers `systemd-detect-virt`
//! prints (`kvm`, `microsoft`, `vmware`, `amazon`, `xen`, `none`, …), so canopy
//! sees a single namespace whichever probe answered.

#[cfg(not(windows))]
use tracing::debug;

/// Detect the virtualisation this host runs under.
///
/// - `Some("none")` — bare metal.
/// - `Some(other)` — the hypervisor, in `systemd-detect-virt`'s vocabulary.
/// - `None` — nothing to go on. **Not** the same as bare metal: it means every
///   probe came up empty, and callers must keep the two apart rather than
///   reporting a host we know nothing about as physical.
///
/// `systemd-detect-virt` goes first where it exists, because it sees things
/// SMBIOS cannot: it reads CPUID as well as DMI, and it recognises containers
/// (`lxc`, `docker`, `podman`, `systemd-nspawn`), which have no firmware of
/// their own to name them. SMBIOS is the fallback that covers Windows, macOS,
/// and Linux hosts without systemd.
///
/// The two sources can name one host slightly differently: CPUID gives the
/// accelerator (`kvm`) where SMBIOS gives the emulator that wrote it (`qemu`).
/// Both are true of the same Proxmox guest.
pub async fn detect_virtualisation() -> Option<String> {
	#[cfg(not(windows))]
	if let Some(virt) = systemd_detect_virt().await {
		return Some(virt);
	}

	smbios::detect().map(str::to_owned)
}

/// Read `systemd-detect-virt`'s output. Returns `None` if the command is
/// unavailable — every systemd host has it, so that's a non-systemd Linux.
///
/// The exit status is deliberately ignored: the command exits non-zero on bare
/// metal, where it still prints the answer we want (`none`) on stdout.
#[cfg(not(windows))]
async fn systemd_detect_virt() -> Option<String> {
	let output = tokio::process::Command::new("systemd-detect-virt")
		.output()
		.await
		.inspect_err(|err| debug!(%err, "systemd-detect-virt is not available"))
		.ok()?;

	let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
	if stdout.is_empty() {
		return None;
	}

	Some(stdout)
}

/// Naming a hypervisor from the SMBIOS system-information strings the firmware
/// handed the OS. On a VM the hypervisor is what populates them, and it names
/// itself.
///
/// `sysinfo::Product` reads these for us on every platform — the SMBIOS table
/// via `GetSystemFirmwareTable` on Windows, `/sys/devices/virtual/dmi/id` on
/// Linux, IOKit on macOS — so this needs no per-platform code, no COM, no WMI
/// service and no elevation.
mod smbios {
	use sysinfo::Product;
	use tracing::debug;

	/// The strings detection matches against, named after the
	/// [`Product`] accessors they come from.
	#[derive(Debug, Default)]
	struct SystemInformation {
		vendor_name: Option<String>,
		name: Option<String>,
		family: Option<String>,
		version: Option<String>,
	}

	impl SystemInformation {
		fn read() -> Self {
			// Blank strings are as good as absent, and firmware writes plenty
			// of them ("To Be Filled By O.E.M.", " ", "").
			let value = |v: Option<String>| v.filter(|s| !s.trim().is_empty());
			Self {
				vendor_name: value(Product::vendor_name()),
				name: value(Product::name()),
				family: value(Product::family()),
				version: value(Product::version()),
			}
		}

		/// Every string joined and lowercased, for substring matching.
		fn haystack(&self) -> String {
			[&self.vendor_name, &self.name, &self.family, &self.version]
				.iter()
				.filter_map(|field| field.as_deref())
				.map(str::to_lowercase)
				.collect::<Vec<_>>()
				.join("\n")
		}

		/// Whether SMBIOS told us anything at all. When it didn't we can't
		/// conclude "bare metal" — we've simply learned nothing.
		fn is_populated(&self) -> bool {
			self.vendor_name.is_some() || self.name.is_some()
		}

		fn field_is(field: &Option<String>, expected: &str) -> bool {
			field
				.as_deref()
				.is_some_and(|value| value.trim().eq_ignore_ascii_case(expected))
		}
	}

	/// Substrings that identify a hypervisor, mapped onto
	/// `systemd-detect-virt`'s identifier for it.
	///
	/// Ordered: the first match wins, so cloud platforms come before the
	/// generic hypervisor they're built on — EC2 Nitro is KVM underneath but
	/// should report `amazon`, while its Xen-era instances name Xen in their
	/// own strings and report `xen`.
	const SIGNATURES: &[(&str, &str)] = &[
		("amazon ec2", "amazon"),
		("google compute engine", "google"),
		("alibaba cloud", "kvm"),
		("nutanix", "kvm"),
		("openstack", "kvm"),
		("ovirt", "kvm"),
		("vmware", "vmware"),
		("virtualbox", "oracle"),
		("innotek gmbh", "oracle"),
		("parallels", "parallels"),
		("apple virtual machine", "apple"),
		("bhyve", "bhyve"),
		("hyper-v", "microsoft"),
		("kvm", "kvm"),
		("qemu", "qemu"),
		("bochs", "bochs"),
		("xen", "xen"),
	];

	/// Detect the hypervisor from SMBIOS.
	///
	/// CPUID is deliberately not consulted as a backstop, even though it's the
	/// more general probe, because on Windows it can't tell guest from host:
	/// the hypervisor-present bit and the `Microsoft Hv` vendor leaf are set on
	/// *physical* hosts running the Hyper-V root partition too — which is any
	/// host with Hyper-V, WSL2, Windows Sandbox or virtualisation-based
	/// security enabled, the last of which Windows turns on by default on much
	/// modern hardware. Separating the two then needs the `CreatePartitions`
	/// privilege bit out of leaf 0x40000003, and reaching CPUID at all needs
	/// `unsafe` or a crate wrapping it. SMBIOS has neither problem: a Hyper-V
	/// host reports its real OEM (Dell, HPE, Lenovo…) while a guest reports
	/// Microsoft. Where CPUID is worth having, `systemd-detect-virt` has
	/// already read it before we get here.
	pub(super) fn detect() -> Option<&'static str> {
		let info = SystemInformation::read();
		debug!(?info, "SMBIOS system information");

		if let Some(id) = identify(&info) {
			return Some(id);
		}

		// SMBIOS named a manufacturer we have no signature for. Like systemd's
		// own DMI table this can be fooled by an unlisted hypervisor, so the
		// strings are logged above for diagnosis.
		if info.is_populated() {
			Some("none")
		} else {
			debug!("SMBIOS system information is empty; virtualisation is unknown");
			None
		}
	}

	/// Identify the hypervisor from SMBIOS strings, or `None` if none matches.
	fn identify(info: &SystemInformation) -> Option<&'static str> {
		// Hyper-V (and Azure, which is Hyper-V) is matched on vendor *and*
		// product together, not by substring: Microsoft ships physical hardware
		// too, and a Surface reports the same vendor string a VM does.
		if SystemInformation::field_is(&info.vendor_name, "microsoft corporation")
			&& SystemInformation::field_is(&info.name, "virtual machine")
		{
			return Some("microsoft");
		}

		let haystack = info.haystack();
		SIGNATURES
			.iter()
			.find_map(|(needle, id)| haystack.contains(needle).then_some(*id))
	}

	#[cfg(test)]
	mod tests {
		use super::*;

		/// Build a `SystemInformation` from `(vendor, product)`, the two fields
		/// every real host populates.
		fn smbios(vendor: &str, product: &str) -> SystemInformation {
			SystemInformation {
				vendor_name: Some(vendor.to_owned()),
				name: Some(product.to_owned()),
				..Default::default()
			}
		}

		#[test]
		fn identifies_hyperv_and_azure_guests() {
			// Both on-prem Hyper-V and Azure present these exact strings, and
			// systemd-detect-virt calls both `microsoft`.
			assert_eq!(
				identify(&smbios("Microsoft Corporation", "Virtual Machine")),
				Some("microsoft"),
			);
		}

		#[test]
		fn microsoft_physical_hardware_is_not_a_vm() {
			// The reason Hyper-V is matched on both fields: Microsoft's own
			// hardware shares the vendor string with its VMs.
			assert_eq!(
				identify(&smbios("Microsoft Corporation", "Surface Laptop 5")),
				None,
			);
		}

		#[test]
		fn identifies_common_hypervisors() {
			for (vendor, product, expected) in [
				("VMware, Inc.", "VMware7,1", "vmware"),
				("VMware, Inc.", "VMware Virtual Platform", "vmware"),
				("innotek GmbH", "VirtualBox", "oracle"),
				("Oracle Corporation", "VirtualBox", "oracle"),
				("QEMU", "Standard PC (Q35 + ICH9, 2009)", "qemu"),
				("Red Hat", "KVM", "kvm"),
				("Xen", "HVM domU", "xen"),
				(
					"Parallels International",
					"Parallels Virtual Platform",
					"parallels",
				),
				("Nutanix", "AHV", "kvm"),
				("OpenStack Foundation", "OpenStack Nova", "kvm"),
				("Google", "Google Compute Engine", "google"),
				("Alibaba Cloud", "Alibaba Cloud ECS", "kvm"),
			] {
				assert_eq!(
					identify(&smbios(vendor, product)),
					Some(expected),
					"{vendor} / {product}",
				);
			}
		}

		#[test]
		fn ec2_wins_over_the_hypervisor_beneath_it() {
			// Nitro instances are KVM underneath but should report `amazon`,
			// the way systemd-detect-virt does.
			assert_eq!(identify(&smbios("Amazon EC2", "t3.medium")), Some("amazon"));
			assert_eq!(identify(&smbios("Xen", "HVM domU")), Some("xen"));
		}

		#[test]
		fn identifies_physical_oems_as_not_virtualised() {
			for (vendor, product) in [
				("Dell Inc.", "PowerEdge R740"),
				("HPE", "ProLiant DL380 Gen10"),
				("LENOVO", "ThinkSystem SR650 V3"),
				("Supermicro", "X11DPi-N"),
				("ASUSTeK COMPUTER INC.", "PRIME B550M-A"),
			] {
				assert_eq!(
					identify(&smbios(vendor, product)),
					None,
					"{vendor} / {product}",
				);
			}
		}

		#[test]
		fn matching_is_case_insensitive() {
			assert_eq!(
				identify(&smbios("MICROSOFT CORPORATION", "VIRTUAL MACHINE")),
				Some("microsoft"),
			);
			assert_eq!(
				identify(&smbios("VMWARE, INC.", "VMWARE7,1")),
				Some("vmware"),
			);
		}

		#[test]
		fn matches_the_version_field_when_the_others_are_generic() {
			// Hyper-V generation 2 guests carry the hypervisor's name in the
			// system version string.
			let info = SystemInformation {
				version: Some("Hyper-V UEFI Release v4.1".to_owned()),
				..Default::default()
			};
			assert_eq!(identify(&info), Some("microsoft"));
		}

		#[test]
		fn empty_smbios_identifies_nothing_and_is_not_populated() {
			let info = SystemInformation::default();
			assert_eq!(identify(&info), None);
			assert!(
				!info.is_populated(),
				"an empty read must stay 'unknown' rather than becoming 'none'",
			);
		}

		#[test]
		fn blank_vendor_does_not_match() {
			// Blank values are filtered at read time, so this shape can't come
			// out of `read` — but the field comparison must reject it too.
			assert!(!SystemInformation::field_is(
				&Some("   ".to_owned()),
				"microsoft corporation"
			));
		}
	}
}

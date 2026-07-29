//! Which hypervisor, if any, this host runs under.
//!
//! One vocabulary across platforms: the identifiers `systemd-detect-virt`
//! prints (`kvm`, `microsoft`, `vmware`, `amazon`, `xen`, `none`, …), so canopy
//! sees a single namespace whichever side did the detecting.

#[cfg(not(windows))]
use tracing::debug;

/// Detect the virtualisation this host runs under.
///
/// - `Some("none")` — bare metal.
/// - `Some(other)` — the hypervisor, in `systemd-detect-virt`'s vocabulary.
/// - `None` — nothing to go on. **Not** the same as bare metal: it means the
///   detection itself came up empty (no `systemd-detect-virt` on a non-systemd
///   Linux, unreadable SMBIOS on Windows, a platform we have no probe for), and
///   callers must keep the two apart rather than reporting a host we know
///   nothing about as physical.
///
/// The two platforms read different sources, so one host can be named slightly
/// differently on each: Linux reads CPUID via systemd and reports the
/// accelerator (`kvm`), while Windows reads SMBIOS and reports the emulator
/// that wrote it (`qemu`). Both are true of the same Proxmox guest.
pub async fn detect_virtualisation() -> Option<String> {
	#[cfg(windows)]
	{
		windows::detect().map(str::to_owned)
	}

	#[cfg(not(windows))]
	{
		systemd_detect_virt().await
	}
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

/// Naming a hypervisor from the SMBIOS strings the firmware handed the OS.
///
/// Only Windows detection reads these, but the matching itself is pure and
/// platform-independent, so it's compiled (and tested) everywhere.
#[cfg(any(windows, test))]
mod smbios {
	/// The SMBIOS system-information strings detection matches against. On a VM
	/// the hypervisor is what populates them, and it names itself.
	#[derive(Debug, Default)]
	pub(super) struct SystemInformation {
		pub system_manufacturer: Option<String>,
		pub system_product_name: Option<String>,
		pub system_family: Option<String>,
		pub bios_vendor: Option<String>,
		pub bios_version: Option<String>,
	}

	impl SystemInformation {
		/// Every string joined and lowercased, for substring matching.
		fn haystack(&self) -> String {
			[
				&self.system_manufacturer,
				&self.system_product_name,
				&self.system_family,
				&self.bios_vendor,
				&self.bios_version,
			]
			.iter()
			.filter_map(|field| field.as_deref())
			.map(str::to_lowercase)
			.collect::<Vec<_>>()
			.join("\n")
		}

		/// Whether SMBIOS told us anything at all. When it didn't we can't
		/// conclude "bare metal" — we've simply learned nothing.
		pub fn is_populated(&self) -> bool {
			self.system_manufacturer.is_some() || self.system_product_name.is_some()
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

	/// Identify the hypervisor from SMBIOS strings, or `None` if none matches.
	pub(super) fn identify(info: &SystemInformation) -> Option<&'static str> {
		// Hyper-V (and Azure, which is Hyper-V) is matched on manufacturer
		// *and* product together, not by substring: Microsoft ships physical
		// hardware too, and a Surface reports the same manufacturer string a
		// VM does.
		if SystemInformation::field_is(&info.system_manufacturer, "microsoft corporation")
			&& SystemInformation::field_is(&info.system_product_name, "virtual machine")
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

		/// Build a `SystemInformation` from `(manufacturer, product)`, the two
		/// fields every real host populates.
		fn smbios(manufacturer: &str, product: &str) -> SystemInformation {
			SystemInformation {
				system_manufacturer: Some(manufacturer.to_owned()),
				system_product_name: Some(product.to_owned()),
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
			// hardware shares the manufacturer string with its VMs.
			assert_eq!(
				identify(&smbios("Microsoft Corporation", "Surface Laptop 5")),
				None,
			);
		}

		#[test]
		fn identifies_common_hypervisors() {
			for (manufacturer, product, expected) in [
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
					identify(&smbios(manufacturer, product)),
					Some(expected),
					"{manufacturer} / {product}",
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
			for (manufacturer, product) in [
				("Dell Inc.", "PowerEdge R740"),
				("HPE", "ProLiant DL380 Gen10"),
				("LENOVO", "ThinkSystem SR650 V3"),
				("Supermicro", "X11DPi-N"),
				("ASUSTeK COMPUTER INC.", "PRIME B550M-A"),
			] {
				assert_eq!(
					identify(&smbios(manufacturer, product)),
					None,
					"{manufacturer} / {product}",
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
		fn matches_bios_fields_when_the_system_fields_are_generic() {
			// Hyper-V generation 2 guests carry the hypervisor's name in the
			// BIOS strings rather than the system ones.
			let info = SystemInformation {
				bios_vendor: Some("Microsoft Corporation".to_owned()),
				bios_version: Some("Hyper-V UEFI Release v4.1".to_owned()),
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
		fn blank_manufacturer_does_not_match() {
			// Blank values are filtered at read time, so this shape can't come
			// from the registry — but the field comparison must reject it too.
			assert!(!SystemInformation::field_is(
				&Some("   ".to_owned()),
				"microsoft corporation"
			));
		}
	}
}

/// Windows has no `systemd-detect-virt`, so read what the firmware told the
/// kernel instead.
#[cfg(windows)]
mod windows {
	use tracing::debug;
	use windows_registry::LOCAL_MACHINE;

	use super::smbios::{SystemInformation, identify};

	/// SMBIOS system-information strings, as the kernel exposes them. Reading
	/// these needs no COM, no WMI service and no elevation — unlike
	/// `Win32_ComputerSystem`, which wants all three from a service context.
	const SYSTEM_INFORMATION: &str = r"SYSTEM\CurrentControlSet\Control\SystemInformation";

	/// Written by the Hyper-V guest integration services, so present only
	/// inside a Hyper-V guest — and, crucially, *absent* on the root partition
	/// (the Hyper-V host itself).
	const HYPERV_GUEST_PARAMETERS: &str = r"SOFTWARE\Microsoft\Virtual Machine\Guest\Parameters";

	/// Detect the hypervisor from the registry.
	///
	/// CPUID is deliberately not consulted, even though it's the more general
	/// probe. On Windows the hypervisor-present bit and the `Microsoft Hv`
	/// vendor leaf are also set on *physical* hosts running the Hyper-V root
	/// partition — which is any host with Hyper-V, WSL2, Windows Sandbox or
	/// virtualisation-based security enabled, the last of which Windows turns
	/// on by default on much modern hardware. Telling root partition from guest
	/// then needs the `CreatePartitions` privilege bit out of CPUID leaf
	/// 0x40000003, and CPUID at all needs `unsafe` or a crate wrapping it.
	/// SMBIOS has neither problem: a Hyper-V host reports its real OEM (Dell,
	/// HPE, Lenovo…) while a guest reports Microsoft.
	pub fn detect() -> Option<&'static str> {
		let info = read_system_information();
		debug!(?info, "SMBIOS system information");

		if let Some(id) = identify(&info) {
			return Some(id);
		}

		// A guest whose SMBIOS was masked (Hyper-V can be told to present the
		// host's own firmware strings) still carries the guest key.
		if hyperv_guest_marker_present() {
			return Some("microsoft");
		}

		// SMBIOS named a manufacturer we don't recognise as a hypervisor. Like
		// systemd's own DMI table this can be fooled by a hypervisor we have no
		// signature for, so the strings are logged above for diagnosis.
		if info.is_populated() {
			Some("none")
		} else {
			debug!("SMBIOS system information is empty; virtualisation is unknown");
			None
		}
	}

	fn read_system_information() -> SystemInformation {
		let Ok(key) = LOCAL_MACHINE.open(SYSTEM_INFORMATION).inspect_err(
			|err| debug!(%err, key = SYSTEM_INFORMATION, "could not open registry key"),
		) else {
			return SystemInformation::default();
		};

		let value = |name: &str| key.get_string(name).ok().filter(|s| !s.trim().is_empty());
		SystemInformation {
			system_manufacturer: value("SystemManufacturer"),
			system_product_name: value("SystemProductName"),
			system_family: value("SystemFamily"),
			bios_vendor: value("BIOSVendor"),
			bios_version: value("BIOSVersion"),
		}
	}

	fn hyperv_guest_marker_present() -> bool {
		LOCAL_MACHINE.open(HYPERV_GUEST_PARAMETERS).is_ok()
	}
}

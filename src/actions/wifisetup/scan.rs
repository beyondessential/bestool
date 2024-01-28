use std::collections::BTreeMap;

use clap::Parser;
use miette::{IntoDiagnostic, Result, miette};
use networkmanager::{devices::{Any, Device, Wireless}, NetworkManager};
use tracing::instrument;

use crate::actions::Context;

use super::WifisetupArgs;

/// Scan for wifi networks.
///
/// This scans for wifi networks and prints the results to stdout. Use `--json` for machine-readable
/// output.
#[derive(Debug, Clone, Parser)]
pub struct ScanArgs {
	/// Print output in JSON format.
	///
	/// Like the human-friendly output, one line is printed per network:
	///
	/// {"ssid": "MyNetwork", "aps": [{"bssid":"00:11:22:33:44:55", "strength": 50, "frequency": 5785, "bitrate": 270000}], "generation": 5, "security": "wpa2", "profile": "uuid"}
	///
	/// The "profile" field is only present if the network is already configured, and is the UUID of
	/// the connection profile.
	#[arg(long)]
	pub json: bool,

	/// Print hidden networks.
	///
	/// By default, hidden networks are not printed. This is because connecting to hidden wifi is
	/// not yet supported. In JSON, the ssid field is an empty string.
	#[arg(long)]
	pub hidden: bool,

	/// Which interface to scan.
	///
	/// By default, the interface is autodetected.
	#[arg(long)]
	pub interface: Option<String>,
}

#[instrument(skip(ctx))]
pub async fn run(ctx: Context<WifisetupArgs, ScanArgs>) -> Result<()> {
	let nm = NetworkManager::new().into_diagnostic()?;

	let mut devs = nm
		.get_devices()
		.into_diagnostic()?
		.into_iter()
		.filter_map(|dev| match dev {
			Device::WiFi(dev) => Some(dev),
			_ => None,
		});

	let dev = if let Some(iface) = ctx.args_sub.interface.clone() {
		devs.find(|dev| dev.interface().map_or(false, |name| name == iface))
	} else {
		devs.next()
	}.ok_or_else(|| miette!("No wifi device found"))?;

	let mut aps = BTreeMap::<String, Vec<Ap>>::new();
	for ap in dev.get_all_access_points().into_diagnostic()? {
		let ssid = ap.ssid().into_diagnostic()?;
		let ap = Ap::try_from(ap)?;
		aps.entry(ssid).or_default().push(ap);
	}

	let aps = aps.into_iter().map(|(ssid, aps)| WifiNetwork { ssid, aps }).collect::<Vec<_>>();

	if ctx.args_sub.json {
		for ap in aps {
			if ap.ssid.is_empty() {
				if !ctx.args_sub.hidden {
					continue;
				}

				for ap in ap.aps {
					println!("{}", serde_json::to_string(&WifiNetwork {
						ssid: "".into(),
						aps: vec![ap],
					}).into_diagnostic()?);
				}
			} else {
				println!("{}", serde_json::to_string(&ap).into_diagnostic()?);
			}
		}
	} else {
		for ap in aps {
			if ap.ssid.is_empty() {
				if !ctx.args_sub.hidden {
					continue;
				}

				println!("\nHidden Networks:");
			} else {
				println!("\nSSID: {}", ap.ssid);
			}

			for ap in ap.aps {
				println!("- BSSID: {}", ap.bssid);
				println!("  Strength: {}", ap.strength);
				println!("  Frequency: {}", ap.frequency);
				println!("  Bitrate: {}", ap.bitrate);
			}
		}
	}

	Ok(())
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq, Ord, PartialOrd, Hash)]
pub struct WifiNetwork {
	pub ssid: String,
	pub aps: Vec<Ap>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq, Ord, PartialOrd, Hash)]
pub struct Ap {
	pub bssid: String,
	pub strength: u8,
	pub frequency: u32,
	pub bitrate: u32,
}

impl TryFrom<networkmanager::AccessPoint> for Ap {
	type Error = miette::Report;

	fn try_from(ap: networkmanager::AccessPoint) -> Result<Self> {
		Ok(Self {
			bssid: ap.hw_address().into_diagnostic()?,
			strength: ap.strength().into_diagnostic()?,
			frequency: ap.frequency().into_diagnostic()?,
			bitrate: ap.max_bitrate().into_diagnostic()?,
		})
	}
}

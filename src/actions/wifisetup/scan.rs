use std::collections::BTreeMap;

use clap::Parser;
use miette::{miette, IntoDiagnostic, Result};
use networkmanager::NetworkManager;
use tracing::instrument;

use crate::actions::Context;

use super::{devices, WifisetupArgs};

/// Scan for wifi networks.
///
/// This scans for wifi networks and prints the results to stdout.
/// Use `--json` for machine-readable output.
#[derive(Debug, Clone, Parser)]
pub struct ScanArgs {
	/// Print output in JSON format.
	///
	/// Like the human-friendly output, one line is printed per network:
	///
	/// {"ssid": "MyNetwork", "aps": [{"bssid":"00:11:22:33:44:55", "strength": 50, "frequency": 5785, "bitrate": 270000}]}
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
	let nm = NetworkManager::new().await.into_diagnostic()?;

	let devs = devices(&nm, ctx.args_sub.interface.as_deref()).await?;
	let dev = devs
		.first()
		.ok_or_else(|| miette!("No wifi device found"))?;

	let mut aps = BTreeMap::<Vec<u8>, Vec<Ap>>::new();
	for ap in dev.get_all_access_points().await.into_diagnostic()? {
		let ssid = ap.ssid().await.into_diagnostic()?;
		let ap = Ap::load(ap).await?;
		aps.entry(ssid).or_default().push(ap);
	}

	let aps = aps
		.into_iter()
		.map(|(ssid, aps)| WifiNetwork { ssid, aps })
		.collect::<Vec<_>>();

	if ctx.args_sub.json {
		for ap in aps {
			if ap.ssid.is_empty() {
				if !ctx.args_sub.hidden {
					continue;
				}

				for ap in ap.aps {
					println!(
						"{}",
						serde_json::to_string(&WifiNetwork {
							ssid: "".into(),
							aps: vec![ap],
						})
						.into_diagnostic()?
					);
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
				let ssid = String::from_utf8_lossy(&ap.ssid);
				println!("\nSSID: {ssid}");
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

#[derive(
	Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq, Ord, PartialOrd, Hash,
)]
pub struct WifiNetwork {
	pub ssid: Vec<u8>,
	pub aps: Vec<Ap>,
}

#[derive(
	Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq, Ord, PartialOrd, Hash,
)]
pub struct Ap {
	pub bssid: String,
	pub strength: u8,
	pub frequency: u32,
	pub bitrate: u32,
}

impl Ap {
	async fn load(ap: networkmanager::device::wireless::AccessPoint) -> Result<Self> {
		Ok(Self {
			bssid: ap.bssid().await.into_diagnostic()?,
			strength: ap.strength().await.into_diagnostic()?,
			frequency: ap.frequency().await.into_diagnostic()?,
			bitrate: ap.max_bitrate().await.into_diagnostic()?,
		})
	}
}

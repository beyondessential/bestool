use std::{net::IpAddr, time::Duration};

use embedded_graphics::{pixelcolor::Rgb565, prelude::*, primitives::Rectangle};
use miette::{IntoDiagnostic, Result, WrapErr};
use tracing::warn;

use crate::actions::iti::display::{Canvas, Widget};

const STROKE: Rgb565 = Rgb565::new(220, 0, 220);
const MAX_LINES: usize = 4;
const WRAP_AT: usize = 26;
const WRAP_INDENT: usize = 2;

pub struct AddressesWidget {
	area: Rectangle,
	last: Option<String>,
}

impl AddressesWidget {
	pub fn new(area: Rectangle) -> Self {
		Self { area, last: None }
	}
}

impl Widget for AddressesWidget {
	fn name(&self) -> &'static str {
		"addresses"
	}

	fn interval(&self) -> Duration {
		Duration::from_secs(60)
	}

	async fn tick(&mut self, canvas: &mut Canvas<'_>) -> Result<()> {
		let hostname = read_hostname();
		let ips = list_global_ipv4()?;
		let mut entries = vec![format!("{hostname}.local")];
		entries.extend(ips.into_iter().take(3).map(|ip| ip.to_string()));

		let lines: Vec<String> = entries
			.into_iter()
			.flat_map(|s| wrap_line(&s).collect::<Vec<_>>())
			.take(MAX_LINES)
			.collect();
		let composed = lines.join("\n");

		if self.last.as_deref() == Some(composed.as_str()) {
			return Ok(());
		}

		canvas.clear_area(self.area)?;
		let baseline_x = self.area.top_left.x;
		let mut baseline_y = self.area.top_left.y + 16;
		for line in &lines {
			canvas.text(Point::new(baseline_x, baseline_y), line, STROKE)?;
			baseline_y += 20;
		}
		self.last = Some(composed);
		Ok(())
	}
}

fn read_hostname() -> String {
	std::fs::read_to_string("/etc/hostname")
		.ok()
		.map(|s| s.trim().to_owned())
		.filter(|s| !s.is_empty())
		.unwrap_or_else(|| "unknown".into())
}

fn list_global_ipv4() -> Result<Vec<IpAddr>> {
	let ifs = if_addrs::get_if_addrs()
		.into_diagnostic()
		.wrap_err("if_addrs: get_if_addrs")?;
	let mut out = Vec::new();
	for iface in ifs {
		if iface.is_loopback() {
			continue;
		}
		if is_excluded(&iface.name) {
			continue;
		}
		let IpAddr::V4(v4) = iface.addr.ip() else {
			continue;
		};
		if v4.is_link_local() || v4.is_unspecified() {
			continue;
		}
		out.push(IpAddr::V4(v4));
	}
	if out.is_empty() {
		warn!("no global IPv4 addresses found");
	}
	Ok(out)
}

fn is_excluded(name: &str) -> bool {
	name.starts_with("podman") || name.starts_with("docker") || name.starts_with("br-")
}

fn wrap_line(s: &str) -> impl Iterator<Item = String> + use<> {
	let mut out = Vec::new();
	if s.len() <= WRAP_AT {
		out.push(s.to_owned());
	} else {
		out.push(s[..WRAP_AT.min(s.len())].to_owned());
		let indent = " ".repeat(WRAP_INDENT);
		out.push(format!("{indent}{}", &s[WRAP_AT.min(s.len())..]));
	}
	out.into_iter()
}

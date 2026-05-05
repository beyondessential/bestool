use std::time::Duration;

use embedded_graphics::{pixelcolor::Rgb565, prelude::*, primitives::Rectangle};
use miette::{IntoDiagnostic, Result, WrapErr};
use tracing::warn;
use zbus::{Connection, proxy};

use crate::actions::iti::display::{Canvas, Widget};

const STROKE: Rgb565 = Rgb565::new(255, 255, 0);
const NM_DEVICE_TYPE_WIFI: u32 = 2;

#[proxy(
	interface = "org.freedesktop.NetworkManager",
	default_service = "org.freedesktop.NetworkManager",
	default_path = "/org/freedesktop/NetworkManager"
)]
trait NetworkManager {
	fn get_devices(&self) -> zbus::Result<Vec<zbus::zvariant::OwnedObjectPath>>;
}

#[proxy(
	interface = "org.freedesktop.NetworkManager.Device",
	default_service = "org.freedesktop.NetworkManager"
)]
trait NmDevice {
	#[zbus(property)]
	fn device_type(&self) -> zbus::Result<u32>;
}

#[proxy(
	interface = "org.freedesktop.NetworkManager.Device.Wireless",
	default_service = "org.freedesktop.NetworkManager"
)]
trait NmWireless {
	#[zbus(property)]
	fn active_access_point(&self) -> zbus::Result<zbus::zvariant::OwnedObjectPath>;
}

#[proxy(
	interface = "org.freedesktop.NetworkManager.AccessPoint",
	default_service = "org.freedesktop.NetworkManager"
)]
trait NmAccessPoint {
	#[zbus(property, name = "Ssid")]
	fn ssid(&self) -> zbus::Result<Vec<u8>>;
}

pub struct WifiWidget {
	area: Rectangle,
	connection: Option<Connection>,
	last: Option<String>,
}

impl WifiWidget {
	pub fn new(area: Rectangle) -> Self {
		Self {
			area,
			connection: None,
			last: None,
		}
	}

	async fn ensure_conn(&mut self) -> Result<&Connection> {
		if self.connection.is_none() {
			let conn = Connection::system()
				.await
				.into_diagnostic()
				.wrap_err("dbus: system bus")?;
			self.connection = Some(conn);
		}
		Ok(self.connection.as_ref().unwrap())
	}

	async fn current_ssid(&mut self) -> Result<Option<String>> {
		let conn = self.ensure_conn().await?.clone();
		let nm = NetworkManagerProxy::new(&conn).await.into_diagnostic()?;
		let devices = nm.get_devices().await.into_diagnostic()?;
		for path in devices {
			let dev = NmDeviceProxy::builder(&conn)
				.path(path.clone())
				.into_diagnostic()?
				.build()
				.await
				.into_diagnostic()?;
			if dev.device_type().await.unwrap_or(0) != NM_DEVICE_TYPE_WIFI {
				continue;
			}
			let wireless = NmWirelessProxy::builder(&conn)
				.path(path)
				.into_diagnostic()?
				.build()
				.await
				.into_diagnostic()?;
			let ap_path = match wireless.active_access_point().await {
				Ok(p) => p,
				Err(_) => return Ok(None),
			};
			if ap_path.as_str() == "/" {
				return Ok(None);
			}
			let ap = NmAccessPointProxy::builder(&conn)
				.path(ap_path)
				.into_diagnostic()?
				.build()
				.await
				.into_diagnostic()?;
			let bytes = ap.ssid().await.into_diagnostic()?;
			return Ok(Some(String::from_utf8_lossy(&bytes).into_owned()));
		}
		Ok(None)
	}
}

impl Widget for WifiWidget {
	fn name(&self) -> &'static str {
		"wifi"
	}

	fn interval(&self) -> Duration {
		Duration::from_secs(60)
	}

	async fn tick(&mut self, canvas: &mut Canvas<'_>) -> Result<()> {
		let ssid = match self.current_ssid().await {
			Ok(s) => s,
			Err(err) => {
				warn!(?err, "querying NetworkManager failed");
				None
			}
		};
		let mut text = format!("Wifi: {}", ssid.as_deref().unwrap_or("not connected"));
		if text.len() > 20 {
			text.truncate(20);
		}
		if self.last.as_deref() == Some(text.as_str()) {
			return Ok(());
		}

		canvas.clear_area(self.area)?;
		let baseline = Point::new(self.area.top_left.x, self.area.top_left.y + 16);
		canvas.text(baseline, &text, STROKE)?;
		self.last = Some(text);
		Ok(())
	}
}

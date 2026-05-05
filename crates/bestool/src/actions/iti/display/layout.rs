use std::str::FromStr;

use embedded_graphics::{prelude::*, primitives::Rectangle};
use miette::{Result, miette};

/// One placeable element on the display.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WidgetKind {
	Clock,
	Addresses,
	Wifi,
	Temperature,
	Battery,
	Sparks,
}

impl WidgetKind {
	pub fn name(self) -> &'static str {
		match self {
			Self::Clock => "clock",
			Self::Addresses => "addresses",
			Self::Wifi => "wifi",
			Self::Temperature => "temperature",
			Self::Battery => "battery",
			Self::Sparks => "sparks",
		}
	}
}

impl FromStr for WidgetKind {
	type Err = miette::Report;

	fn from_str(s: &str) -> Result<Self> {
		match s {
			"clock" => Ok(Self::Clock),
			"addresses" => Ok(Self::Addresses),
			"wifi" => Ok(Self::Wifi),
			"temperature" => Ok(Self::Temperature),
			"battery" => Ok(Self::Battery),
			"sparks" => Ok(Self::Sparks),
			other => Err(miette!(
				"unknown widget {other:?}; valid: clock, addresses, wifi, temperature, battery, sparks"
			)),
		}
	}
}

/// Layout entry: one widget, its area on the panel, and its default refresh interval in seconds.
pub struct LayoutEntry {
	pub kind: WidgetKind,
	pub area: Rectangle,
	pub interval_secs: u64,
}

const fn rect(x: i32, y: i32, w: u32, h: u32) -> Rectangle {
	Rectangle::new(Point::new(x, y), Size::new(w, h))
}

/// Pixel placement for every widget. Positions match the previous deployment so visual
/// regression on the device is minimal.
///
/// Panel coordinates: 280 wide × 240 tall (landscape).
pub const LAYOUT: &[LayoutEntry] = &[
	LayoutEntry {
		kind: WidgetKind::Clock,
		area: rect(100, 4, 160, 20),
		interval_secs: 10,
	},
	LayoutEntry {
		kind: WidgetKind::Sparks,
		area: rect(10, 30, 260, 27),
		interval_secs: 10,
	},
	LayoutEntry {
		kind: WidgetKind::Addresses,
		area: rect(10, 65, 260, 80),
		interval_secs: 60,
	},
	LayoutEntry {
		kind: WidgetKind::Wifi,
		area: rect(10, 180, 200, 20),
		interval_secs: 60,
	},
	LayoutEntry {
		kind: WidgetKind::Temperature,
		area: rect(218, 180, 62, 20),
		interval_secs: 10,
	},
	LayoutEntry {
		kind: WidgetKind::Battery,
		area: rect(18, 205, 254, 20),
		interval_secs: 10,
	},
];

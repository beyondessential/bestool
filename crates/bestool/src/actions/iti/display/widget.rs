use std::{future::Future, time::Duration};

use embedded_graphics::primitives::Rectangle;
use miette::Result;

use super::canvas::Canvas;

/// One ticking display element (clock, battery readout, spark lines, ...).
///
/// The harness calls [`Widget::tick`] every [`Widget::interval`]. Widgets own their sampling
/// state and re-draw their entire [`Widget::area`] on each tick. Rendering is sequential, so
/// widgets don't have to worry about contention on the LCD.
pub trait Widget: Send + 'static {
	/// Stable identifier; used for `--disable` flags and logging.
	fn name(&self) -> &'static str;

	/// How often to call [`Widget::tick`].
	fn interval(&self) -> Duration;

	/// Pixel rectangle the widget owns.
	fn area(&self) -> Rectangle;

	/// Sample current state and re-draw the widget's area.
	fn tick(&mut self, canvas: &mut Canvas<'_>) -> impl Future<Output = Result<()>> + Send;
}

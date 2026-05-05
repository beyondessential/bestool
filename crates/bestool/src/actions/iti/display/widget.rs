use std::{future::Future, pin::Pin, time::Duration};

use miette::Result;

use super::canvas::Canvas;

/// One ticking display element (clock, battery readout, spark lines, ...).
///
/// The harness calls [`Widget::tick`] every [`Widget::interval`]. Widgets own their sampling
/// state and the rectangle they were constructed with, and re-draw that rectangle on each tick.
/// Rendering is sequential, so widgets don't have to worry about contention on the LCD.
pub trait Widget: Send + 'static {
	/// Stable identifier; used for `--disable` flags and logging.
	fn name(&self) -> &'static str;

	/// How often to call [`Widget::tick`].
	fn interval(&self) -> Duration;

	/// Sample current state and re-draw the widget's area.
	fn tick(&mut self, canvas: &mut Canvas<'_>) -> impl Future<Output = Result<()>> + Send;
}

/// Object-safe wrapper for [`Widget`], used by the layout machinery to store heterogeneous
/// widgets behind a common interface. Implemented blanket-style for every `Widget`.
pub trait DynWidget: Send {
	fn name(&self) -> &'static str;
	fn interval(&self) -> Duration;
	fn tick<'a>(
		&'a mut self,
		canvas: &'a mut Canvas<'a>,
	) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>>;
}

impl<W: Widget> DynWidget for W {
	fn name(&self) -> &'static str {
		Widget::name(self)
	}
	fn interval(&self) -> Duration {
		Widget::interval(self)
	}
	fn tick<'a>(
		&'a mut self,
		canvas: &'a mut Canvas<'a>,
	) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
		Box::pin(Widget::tick(self, canvas))
	}
}

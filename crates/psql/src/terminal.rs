//! Terminal size detection and resize handling

use portable_pty::PtySize;
use std::sync::{Arc, Mutex};
use std::thread;
use tracing::{debug, warn};

#[cfg(windows)]
use std::time::Duration;

#[cfg(unix)]
use signal_hook::consts::SIGWINCH;
#[cfg(unix)]
use signal_hook::iterator::Signals;

#[cfg(windows)]
use windows_sys::Win32::Foundation::HANDLE;
#[cfg(windows)]
use windows_sys::Win32::System::Console::{
	GetConsoleScreenBufferInfo, GetStdHandle, CONSOLE_SCREEN_BUFFER_INFO, STD_OUTPUT_HANDLE,
};

/// Get the current terminal size, or default to 80x24
pub fn get_terminal_size() -> (u16, u16) {
	let (cols, rows) = terminal_size::terminal_size()
		.map(|(w, h)| (w.0, h.0))
		.unwrap_or((80, 24));

	debug!(cols, rows, "terminal size detected");
	(cols, rows)
}

/// Spawn a background thread to handle terminal resize events
///
/// On Unix, this uses SIGWINCH signals to detect resize events.
/// On Windows, this polls the console buffer info periodically.
pub fn spawn_resize_handler(pty_master: Arc<Mutex<Box<dyn portable_pty::MasterPty + Send>>>) {
	#[cfg(unix)]
	{
		thread::spawn(move || {
			debug!("starting SIGWINCH handler thread");
			let mut signals = match Signals::new([SIGWINCH]) {
				Ok(s) => s,
				Err(e) => {
					warn!("failed to register SIGWINCH handler: {}", e);
					return;
				}
			};

			for _ in signals.forever() {
				if let Some((w, h)) = terminal_size::terminal_size() {
					let new_size = PtySize {
						rows: h.0,
						cols: w.0,
						pixel_width: 0,
						pixel_height: 0,
					};

					debug!(cols = w.0, rows = h.0, "terminal resized (SIGWINCH)");
					if let Ok(master) = pty_master.lock() {
						let _ = master.resize(new_size);
					}
				}
			}
		});
	}

	#[cfg(windows)]
	{
		thread::spawn(move || unsafe {
			debug!("starting Windows console resize handler thread");
			let stdout_handle: HANDLE = GetStdHandle(STD_OUTPUT_HANDLE);
			if stdout_handle == 0 || stdout_handle == -1i32 as HANDLE {
				warn!("failed to get stdout handle for resize detection");
				return;
			}

			let (initial_cols, initial_rows) = get_terminal_size();
			let mut last_size = (initial_cols, initial_rows);

			loop {
				thread::sleep(Duration::from_millis(200));

				let mut csbi: CONSOLE_SCREEN_BUFFER_INFO = std::mem::zeroed();
				if GetConsoleScreenBufferInfo(stdout_handle, &mut csbi) == 0 {
					continue;
				}

				let new_cols = (csbi.srWindow.Right - csbi.srWindow.Left + 1) as u16;
				let new_rows = (csbi.srWindow.Bottom - csbi.srWindow.Top + 1) as u16;

				if (new_cols, new_rows) != last_size {
					last_size = (new_cols, new_rows);

					let new_size = PtySize {
						rows: new_rows,
						cols: new_cols,
						pixel_width: 0,
						pixel_height: 0,
					};

					debug!(
						cols = new_cols,
						rows = new_rows,
						"terminal resized (Windows)"
					);
					if let Ok(master) = pty_master.lock() {
						let _ = master.resize(new_size);
					}
				}
			}
		});
	}

	#[cfg(not(any(unix, windows)))]
	{
		// Suppress unused variable warning on unsupported platforms
		let _ = pty_master;
		warn!("terminal resize handling not implemented for this platform");
	}
}

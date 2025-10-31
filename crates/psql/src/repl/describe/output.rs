use std::sync::Arc;

use tokio::{
	fs::File,
	io::{self, AsyncWriteExt},
	sync::Mutex as TokioMutex,
};

pub(super) struct OutputWriter {
	file: Option<Arc<TokioMutex<File>>>,
}

impl OutputWriter {
	pub fn new(file: Option<Arc<TokioMutex<File>>>) -> Self {
		Self { file }
	}

	pub async fn write(&self, msg: &str) {
		if let Some(ref file_arc) = self.file {
			let mut file = file_arc.lock().await;
			let _ = file.write_all(msg.as_bytes()).await;
			let _ = file.flush().await;
		} else {
			let mut stdout = io::stdout();
			let _ = stdout.write_all(msg.as_bytes()).await;
			let _ = stdout.flush().await;
		}
	}

	pub async fn writeln(&self, msg: &str) {
		self.write(&format!("{}\n", msg)).await;
	}
}

#[macro_export]
macro_rules! write_output {
	($writer:expr, $($arg:tt)*) => {{
		$writer.write(&format!($($arg)*)).await;
	}};
}

#[macro_export]
macro_rules! writeln_output {
	($writer:expr, $($arg:tt)*) => {{
		$writer.writeln(&format!($($arg)*)).await;
	}};
}

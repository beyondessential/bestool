use std::path::{Path, PathBuf};

use miette::{miette, IntoDiagnostic, Result};

#[derive(Debug, Clone)]
pub struct Snippets {
	savedir: Option<PathBuf>,
	dirs: Vec<PathBuf>,
}

impl Snippets {
	pub fn new() -> Self {
		let mut savedir = None;
		let mut dirs = Vec::new();
		if let Some(dir) = dirs::data_local_dir() {
			let dir = dir.join("snippets");
			savedir = Some(dir.clone());
			dirs.push(dir);
		}
		if let Some(dir) = dirs::data_dir() {
			let dir = dir.join("snippets");
			savedir = Some(dir.clone());
			dirs.push(dir);
		}
		if let Some(dir) = dirs::config_local_dir() {
			dirs.push(dir.join("snippets"));
		}
		if let Some(dir) = dirs::config_dir() {
			dirs.push(dir.join("snippets"));
		}
		dirs.push({
			let dir = PathBuf::from("/tamanu/snippets");
			if dir.exists() {
				savedir = Some(dir.clone());
			}
			dir
		});
		dirs.push({
			let dir = PathBuf::from("/snippets");
			if dir.exists() {
				savedir = Some(dir.clone());
			}
			dir
		});
		dirs.push("/etc/bestool/snippets".into());
		Self { savedir, dirs }
	}

	#[cfg(test)]
	pub fn empty() -> Self {
		Self {
			savedir: None,
			dirs: Vec::new(),
		}
	}

	fn try_path(dir: &Path, name: &str) -> Option<PathBuf> {
		let path = dir.join(&format!("{name}.sql"));
		if path.exists() {
			Some(path)
		} else {
			None
		}
	}

	pub fn path(&self, name: &str) -> Result<PathBuf> {
		for dir in &self.dirs {
			if let Some(path) = Self::try_path(dir, name) {
				return Ok(path);
			}
		}

		Err(miette!("Snippet '{name}' not found"))
	}

	pub async fn save(&self, name: &str, content: &str) -> Result<()> {
		let path = self.savedir.as_ref().ok_or(miette!("No savedir"))?;
		let path = path.join(&format!("{name}.sql"));
		tokio::fs::write(&path, content).await.into_diagnostic()?;
		Ok(())
	}
}

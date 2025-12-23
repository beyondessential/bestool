use std::path::{Path, PathBuf};

use miette::{IntoDiagnostic, Result, miette};

#[derive(Debug, Clone)]
pub struct Snippets {
	savedir: Option<PathBuf>,
	pub dirs: Vec<PathBuf>,
}

impl Default for Snippets {
	fn default() -> Self {
		Self::new()
	}
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

	pub fn empty() -> Self {
		Self {
			savedir: None,
			dirs: Vec::new(),
		}
	}

	#[cfg(test)]
	pub fn with_savedir(savedir: PathBuf) -> Self {
		Self {
			savedir: Some(savedir.clone()),
			dirs: vec![savedir],
		}
	}

	fn try_path(dir: &Path, name: &str) -> Option<PathBuf> {
		let path = dir.join(format!("{name}.sql"));
		if path.exists() { Some(path) } else { None }
	}

	pub fn path(&self, name: &str) -> Result<PathBuf> {
		for dir in &self.dirs {
			if let Some(path) = Self::try_path(dir, name) {
				return Ok(path);
			}
		}

		Err(miette!("Snippet '{name}' not found"))
	}

	pub fn lookup_with_fallback(
		&self,
		name: &str,
		lookup: Option<&crate::config::SnippetLookup>,
	) -> Result<String> {
		for dir in &self.dirs {
			if let Some(path) = Self::try_path(dir, name) {
				if let Ok(content) = std::fs::read_to_string(&path) {
					return Ok(content);
				}
			}
		}

		if let Some(lookup_provider) = lookup {
			if let Some(content) = lookup_provider.lookup(name) {
				return Ok(content);
			}
		}

		Err(miette!("Snippet '{name}' not found"))
	}

	pub async fn save(&self, name: &str, content: &str) -> Result<PathBuf> {
		let savedir = self.savedir.as_ref().ok_or(miette!("No savedir"))?;
		tokio::fs::create_dir_all(savedir).await.into_diagnostic()?;
		let path = savedir.join(format!("{name}.sql"));
		tokio::fs::write(&path, content).await.into_diagnostic()?;
		Ok(path)
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use tempfile::TempDir;

	#[test]
	fn test_empty_snippets() {
		let snippets = Snippets::empty();
		let result = snippets.path("nonexistent");
		assert!(result.is_err());
		assert_eq!(
			result.unwrap_err().to_string(),
			"Snippet 'nonexistent' not found"
		);
	}

	#[tokio::test]
	async fn test_save_creates_savedir() {
		let temp_dir = TempDir::new().unwrap();
		let savedir = temp_dir.path().join("snippets");
		assert!(!savedir.exists());

		let snippets = Snippets::with_savedir(savedir.clone());
		let saved_path = snippets.save("test_snippet", "SELECT 1;").await.unwrap();

		assert!(savedir.exists());
		let saved_file = savedir.join("test_snippet.sql");
		assert!(saved_file.exists());
		assert_eq!(saved_path, saved_file);

		let content = tokio::fs::read_to_string(&saved_file).await.unwrap();
		assert_eq!(content, "SELECT 1;");
	}

	#[tokio::test]
	async fn test_save_overwrites_existing_snippet() {
		let temp_dir = TempDir::new().unwrap();
		let savedir = temp_dir.path().to_path_buf();

		let snippets = Snippets::with_savedir(savedir.clone());
		let path1 = snippets.save("test_snippet", "SELECT 1;").await.unwrap();
		let path2 = snippets.save("test_snippet", "SELECT 2;").await.unwrap();

		assert_eq!(path1, path2);
		let saved_file = savedir.join("test_snippet.sql");
		let content = tokio::fs::read_to_string(&saved_file).await.unwrap();
		assert_eq!(content, "SELECT 2;");
	}

	#[tokio::test]
	async fn test_save_multiple_snippets() {
		let temp_dir = TempDir::new().unwrap();
		let savedir = temp_dir.path().to_path_buf();

		let snippets = Snippets::with_savedir(savedir.clone());
		let path1 = snippets.save("snippet1", "SELECT 1;").await.unwrap();
		let path2 = snippets.save("snippet2", "SELECT 2;").await.unwrap();
		let path3 = snippets.save("snippet3", "SELECT 3;").await.unwrap();

		assert_eq!(path1, savedir.join("snippet1.sql"));
		assert_eq!(path2, savedir.join("snippet2.sql"));
		assert_eq!(path3, savedir.join("snippet3.sql"));

		assert!(savedir.join("snippet1.sql").exists());
		assert!(savedir.join("snippet2.sql").exists());
		assert!(savedir.join("snippet3.sql").exists());

		let content1 = tokio::fs::read_to_string(savedir.join("snippet1.sql"))
			.await
			.unwrap();
		assert_eq!(content1, "SELECT 1;");
	}

	#[tokio::test]
	async fn test_save_no_savedir_fails() {
		let snippets = Snippets::empty();
		let result = snippets.save("test_snippet", "SELECT 1;").await;
		assert!(result.is_err());
		assert_eq!(result.unwrap_err().to_string(), "No savedir");
	}

	#[test]
	fn test_path_finds_existing_snippet() {
		let temp_dir = TempDir::new().unwrap();
		let savedir = temp_dir.path().to_path_buf();

		let snippet_file = savedir.join("test.sql");
		std::fs::write(&snippet_file, "SELECT 1;").unwrap();

		let snippets = Snippets::with_savedir(savedir);
		let result = snippets.path("test").unwrap();
		assert_eq!(result, snippet_file);
	}

	#[test]
	fn test_path_returns_error_for_missing_snippet() {
		let temp_dir = TempDir::new().unwrap();
		let savedir = temp_dir.path().to_path_buf();

		let snippets = Snippets::with_savedir(savedir);
		let result = snippets.path("nonexistent");
		assert!(result.is_err());
	}

	#[tokio::test]
	async fn test_save_with_special_characters_in_name() {
		let temp_dir = TempDir::new().unwrap();
		let savedir = temp_dir.path().to_path_buf();

		let snippets = Snippets::with_savedir(savedir.clone());
		let saved_path = snippets
			.save("test_snippet-123", "SELECT 1;")
			.await
			.unwrap();

		let saved_file = savedir.join("test_snippet-123.sql");
		assert!(saved_file.exists());
		assert_eq!(saved_path, saved_file);
	}

	#[tokio::test]
	async fn test_save_with_multiline_content() {
		let temp_dir = TempDir::new().unwrap();
		let savedir = temp_dir.path().to_path_buf();

		let snippets = Snippets::with_savedir(savedir.clone());
		let content = "SELECT *\nFROM users\nWHERE id = 1;";
		let saved_path = snippets.save("multiline", content).await.unwrap();

		let saved_file = savedir.join("multiline.sql");
		assert_eq!(saved_path, saved_file);
		let read_content = tokio::fs::read_to_string(&saved_file).await.unwrap();
		assert_eq!(read_content, content);
	}

	#[tokio::test]
	async fn test_save_preserves_content_exactly() {
		let temp_dir = TempDir::new().unwrap();
		let savedir = temp_dir.path().to_path_buf();

		let snippets = Snippets::with_savedir(savedir.clone());
		let content = "-- Comment\nSELECT 1; -- inline comment\n";
		let saved_path = snippets.save("with_comments", content).await.unwrap();

		let saved_file = savedir.join("with_comments.sql");
		assert_eq!(saved_path, saved_file);
		let read_content = tokio::fs::read_to_string(&saved_file).await.unwrap();
		assert_eq!(read_content, content);
	}
}

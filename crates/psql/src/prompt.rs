//! Prompt parsing and formatting for psql
//!
//! This module handles the custom prompt boundary markers that we inject into psql
//! to detect when we're at a prompt and to extract prompt information.

/// Information parsed from a psql prompt
#[derive(Debug, Clone)]
pub struct PromptInfo {
	pub database: String,
	#[allow(dead_code)]
	pub username: String,
	pub user_type: String,   // "#" for superuser, ">" for regular
	pub status: String,      // "=" normal, "!" disconnected, "^" single-line
	pub transaction: String, // "" none, "*" in transaction, "!" failed transaction, "?" unknown
	pub prompt_type: u8,     // 1 = PROMPT1 (normal), 2 = PROMPT2 (continuation), 3 = PROMPT3 (COPY)
}

impl PromptInfo {
	/// Parse from our custom format: <<<BOUNDARY|||type|||db|||user|||usertype|||status|||transaction>>>
	pub fn parse(line: &str, boundary: &str) -> Option<Self> {
		let marker_start = format!("<<<{}|||", boundary);
		let marker_end = ">>>";

		let start = line.find(&marker_start)?;
		let end = line.find(marker_end)?;

		if end <= start {
			return None;
		}

		let content = &line[start + marker_start.len()..end];
		let parts: Vec<&str> = content.split("|||").collect();

		if parts.len() != 6 {
			return None;
		}

		let prompt_type = parts[0].parse::<u8>().ok()?;

		Some(PromptInfo {
			database: parts[1].to_string(),
			username: parts[2].to_string(),
			user_type: parts[3].to_string(),
			status: parts[4].to_string(),
			transaction: parts[5].to_string(),
			prompt_type,
		})
	}

	/// Format as a standard psql prompt
	pub fn format_prompt(&self) -> String {
		match self.prompt_type {
			2 => {
				// PROMPT2: continuation prompt (multi-line queries)
				format!(
					"{}{}{}{} ",
					self.database, self.status, self.transaction, "-"
				)
			}
			3 => {
				// PROMPT3: COPY mode prompt
				">> ".to_string()
			}
			_ => {
				// PROMPT1: normal prompt
				format!(
					"{}{}{}{} ",
					self.database, self.status, self.transaction, self.user_type
				)
			}
		}
	}

	/// Check if currently in a transaction
	pub fn in_transaction(&self) -> bool {
		!self.transaction.is_empty()
	}

	/// Get a description of the transaction state
	pub fn transaction_state_description(&self) -> &str {
		match self.transaction.as_str() {
			"*" => "in transaction",
			"!" => "in failed transaction",
			"?" => "in unknown transaction state",
			_ => "no transaction",
		}
	}
}

/// Generate a random boundary marker for prompt detection
pub fn generate_boundary() -> String {
	use rand::Rng;
	use std::fmt::Write;

	let mut rng = rand::thread_rng();
	let random_bytes: [u8; 16] = rng.gen();

	let mut result = String::with_capacity(32);
	for byte in random_bytes {
		write!(&mut result, "{:02x}", byte).unwrap();
	}
	result
}

use super::transaction::TransactionState;

pub fn build_prompt(
	database_name: &str,
	is_superuser: bool,
	buffer_is_empty: bool,
	transaction_state: TransactionState,
	write_mode: bool,
) -> String {
	let (transaction_marker, color_code) = match transaction_state {
		TransactionState::Error => ("!", "\x01\x1b[1;31m\x02"), // Bold red
		TransactionState::Active => {
			if write_mode {
				("*", "\x01\x1b[1;34m\x02") // Bold blue (write mode + transaction)
			} else {
				("*", "") // No color (read mode + transaction)
			}
		}
		TransactionState::Idle => {
			if write_mode {
				("", "\x01\x1b[1;32m\x02") // Bold green (write mode + idle transaction)
			} else {
				("", "") // No color (read mode + idle transaction)
			}
		}
		TransactionState::None => {
			if write_mode {
				("", "\x01\x1b[1;32m\x02") // Bold green (write mode, no transaction)
			} else {
				("", "") // No color (read mode, no transaction)
			}
		}
	};

	let reset_code = if color_code.is_empty() {
		""
	} else {
		"\x01\x1b[0m\x02"
	};
	let prompt_suffix = if is_superuser { "#" } else { ">" };

	if buffer_is_empty {
		format!(
			"{}{}={}{}{} ",
			color_code, database_name, transaction_marker, prompt_suffix, reset_code
		)
	} else {
		format!("{}{}->{}  ", color_code, database_name, reset_code)
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_prompt_without_colors_no_markers() {
		let prompt = build_prompt("testdb", false, true, TransactionState::None, false);
		assert_eq!(prompt, "testdb=> ");
		// Should not contain any escape codes or markers
		assert!(!prompt.contains('\x01'));
		assert!(!prompt.contains('\x02'));
		assert!(!prompt.contains('\x1b'));
	}

	#[test]
	fn test_prompt_with_colors_has_markers() {
		let prompt = build_prompt("testdb", false, true, TransactionState::None, true);
		// Should contain markers around ANSI codes
		assert!(prompt.contains('\x01'));
		assert!(prompt.contains('\x02'));
		// Should have matched pairs of markers
		let marker_start_count = prompt.chars().filter(|&c| c == '\x01').count();
		let marker_end_count = prompt.chars().filter(|&c| c == '\x02').count();
		assert_eq!(marker_start_count, marker_end_count);
	}

	#[test]
	fn test_write_mode_prompt_has_wrapped_escape_codes() {
		let prompt = build_prompt("mydb", false, true, TransactionState::None, true);
		// Verify the pattern: \x01\x1b[...m\x02
		assert!(prompt.contains("\x01\x1b[1;32m\x02")); // Bold green start
		assert!(prompt.contains("\x01\x1b[0m\x02")); // Reset
	}

	#[test]
	fn test_error_state_prompt_has_wrapped_escape_codes() {
		let prompt = build_prompt("mydb", false, true, TransactionState::Error, false);
		// Error state should have bold red wrapped in markers
		assert!(prompt.contains("\x01\x1b[1;31m\x02"));
		assert!(prompt.contains("\x01\x1b[0m\x02"));
	}

	#[test]
	fn test_continuation_prompt_with_colors() {
		let prompt = build_prompt("mydb", false, false, TransactionState::None, true);
		// Continuation prompt: mydb->
		assert!(prompt.contains("->"));
		assert!(prompt.contains('\x01'));
		assert!(prompt.contains('\x02'));
	}

	#[test]
	fn test_superuser_prompt() {
		let prompt = build_prompt("postgres", true, true, TransactionState::None, false);
		assert!(prompt.contains('#'));
		assert!(prompt.ends_with("# "));
	}

	#[test]
	fn test_transaction_markers() {
		let prompt_active = build_prompt("testdb", false, true, TransactionState::Active, false);
		assert!(prompt_active.contains("=*>"));

		let prompt_error = build_prompt("testdb", false, true, TransactionState::Error, false);
		assert!(prompt_error.contains("=!>"));
	}
}

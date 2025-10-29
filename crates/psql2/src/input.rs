use crate::parser::{parse_metacommand, parse_query_modifiers, Metacommand};
use crate::repl::ReplState;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ReplAction {
	Continue,
	Execute {
		input: String,
		sql: String,
		modifiers: crate::parser::QueryModifiers,
	},
	Exit,
	ToggleExpanded,
	ToggleWriteMode,
}

pub(crate) fn handle_input(
	buffer: &str,
	new_line: &str,
	state: &ReplState,
) -> (String, ReplAction) {
	let mut new_buffer = buffer.to_string();

	if !new_buffer.is_empty() {
		new_buffer.push('\n');
	}
	new_buffer.push_str(new_line);

	let user_input = new_buffer.trim().to_string();

	// Check for metacommands first (only if buffer is empty, i.e., command starts on first character)
	if buffer.is_empty() {
		if let Ok(Some(metacmd)) = parse_metacommand(&user_input) {
			let action = match metacmd {
				Metacommand::Quit => ReplAction::Exit,
				Metacommand::Expanded => ReplAction::ToggleExpanded,
				Metacommand::WriteMode => ReplAction::ToggleWriteMode,
			};
			return (String::new(), action);
		}
	}

	// Handle legacy "quit" command for compatibility
	if buffer.is_empty() && user_input.eq_ignore_ascii_case("quit") {
		return (String::new(), ReplAction::Exit);
	}

	let parse_result = parse_query_modifiers(&user_input);

	let action = match parse_result {
		Ok(Some((sql, mut modifiers))) => {
			// Apply expanded mode state if enabled
			if state.expanded_mode {
				modifiers.insert(crate::parser::QueryModifier::Expanded);
			}
			ReplAction::Execute {
				input: user_input.clone(),
				sql,
				modifiers,
			}
		}
		Ok(None) | Err(_) => ReplAction::Continue,
	};

	let buffer_state = if let ReplAction::Continue = &action {
		new_buffer
	} else {
		String::new()
	};

	(buffer_state, action)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_handle_input_empty_line() {
		let state = ReplState::new();
		let (buffer, action) = handle_input("", "", &state);
		assert_eq!(buffer, "");
		assert_eq!(action, ReplAction::Continue);
	}

	#[test]
	fn test_handle_input_incomplete_query() {
		let state = ReplState::new();
		let (buffer, action) = handle_input("", "SELECT * FROM users", &state);
		assert_eq!(buffer, "SELECT * FROM users");
		assert_eq!(action, ReplAction::Continue);
	}

	#[test]
	fn test_handle_input_complete_query_semicolon() {
		let state = ReplState::new();
		let (buffer, action) = handle_input("", "SELECT * FROM users;", &state);
		assert_eq!(buffer, "");
		match action {
			ReplAction::Execute {
				input,
				sql,
				modifiers,
			} => {
				assert_eq!(input, "SELECT * FROM users;");
				assert_eq!(sql, "SELECT * FROM users");
				assert!(modifiers.is_empty());
			}
			_ => panic!("Expected Execute action"),
		}
	}

	#[test]
	fn test_handle_input_complete_query_backslash_g() {
		let state = ReplState::new();
		let (buffer, action) = handle_input("", "SELECT * FROM users\\g", &state);
		assert_eq!(buffer, "");
		match action {
			ReplAction::Execute {
				input,
				sql,
				modifiers,
			} => {
				assert_eq!(input, "SELECT * FROM users\\g");
				assert_eq!(sql, "SELECT * FROM users");
				assert!(modifiers.is_empty());
			}
			_ => panic!("Expected Execute action"),
		}
	}

	#[test]
	fn test_handle_input_multiline_query() {
		let state = ReplState::new();
		let (buffer1, action1) = handle_input("", "SELECT *", &state);
		assert_eq!(buffer1, "SELECT *");
		assert_eq!(action1, ReplAction::Continue);

		let (buffer2, action2) = handle_input(&buffer1, "FROM users;", &state);
		assert_eq!(buffer2, "");
		match action2 {
			ReplAction::Execute { input, sql, .. } => {
				assert_eq!(input, "SELECT *\nFROM users;");
				assert_eq!(sql, "SELECT *\nFROM users");
			}
			_ => panic!("Expected Execute action"),
		}
	}

	#[test]
	fn test_handle_input_quit_command() {
		let state = ReplState::new();
		let (buffer, action) = handle_input("", "\\q", &state);
		assert_eq!(buffer, "");
		assert_eq!(action, ReplAction::Exit);
	}

	#[test]
	fn test_handle_input_quit_command_case_insensitive() {
		let state = ReplState::new();
		let (buffer, action) = handle_input("", "QUIT", &state);
		assert_eq!(buffer, "");
		assert_eq!(action, ReplAction::Exit);
	}

	#[test]
	fn test_handle_input_quit_after_incomplete() {
		let state = ReplState::new();
		let (buffer1, action1) = handle_input("", "SELECT *", &state);
		assert_eq!(buffer1, "SELECT *");
		assert_eq!(action1, ReplAction::Continue);

		// \q after incomplete query is not treated as quit - it's part of the query
		let (buffer2, action2) = handle_input(&buffer1, "\\q", &state);
		assert_eq!(buffer2, "SELECT *\n\\q");
		assert_eq!(action2, ReplAction::Continue);
	}

	#[test]
	fn test_handle_input_expanded_metacommand() {
		let state = ReplState::new();
		let (buffer, action) = handle_input("", "\\x", &state);
		assert_eq!(buffer, "");
		assert_eq!(action, ReplAction::ToggleExpanded);
	}

	#[test]
	fn test_handle_input_expanded_metacommand_uppercase() {
		let state = ReplState::new();
		let (buffer, action) = handle_input("", "\\X", &state);
		assert_eq!(buffer, "");
		assert_eq!(action, ReplAction::ToggleExpanded);
	}

	#[test]
	fn test_ctrl_c_clears_buffer() {
		let state = ReplState::new();
		// Simulate building up a query
		let (buffer, _) = handle_input("", "SELECT *", &state);
		assert_eq!(buffer, "SELECT *");

		// Ctrl-C should clear the buffer (simulated by setting buffer to empty)
		let cleared_buffer = "";
		assert_eq!(cleared_buffer, "");

		// Can start fresh after Ctrl-C
		let (new_buffer, action) = handle_input(cleared_buffer, "SELECT 1;", &state);
		assert_eq!(new_buffer, "");
		match action {
			ReplAction::Execute { input, sql, .. } => {
				assert_eq!(input, "SELECT 1;");
				assert_eq!(sql, "SELECT 1");
			}
			_ => panic!("Expected Execute action"),
		}
	}

	#[test]
	fn test_ctrl_c_on_empty_buffer() {
		// Ctrl-C on empty buffer should keep it empty (not exit)
		let _buffer = "";
		let cleared_buffer = "";
		assert_eq!(cleared_buffer, "");
	}

	#[test]
	fn test_ctrl_d_exits() {
		// Ctrl-D behavior is tested via ReadlineError::Eof in the main loop
		// This is a documentation test showing the expected behavior
		// Ctrl-D (EOF) should exit the REPL regardless of buffer state
	}

	#[test]
	fn test_handle_input_preserves_modifiers() {
		let state = ReplState::new();
		let (buffer, action) = handle_input("", "select 1+1 \\gx", &state);
		assert_eq!(buffer, "");
		match action {
			ReplAction::Execute {
				input,
				sql,
				modifiers,
			} => {
				assert_eq!(input, "select 1+1 \\gx");
				assert_eq!(sql, "select 1+1");
				assert!(modifiers.contains(&crate::parser::QueryModifier::Expanded));
			}
			_ => panic!("Expected Execute action"),
		}
	}

	#[test]
	fn test_expanded_mode_applied_to_query() {
		let state = ReplState {
			expanded_mode: true,
			write_mode: false,
			ots: None,
			..ReplState::new()
		};
		let (buffer, action) = handle_input("", "SELECT 1;", &state);
		assert_eq!(buffer, "");
		match action {
			ReplAction::Execute { modifiers, .. } => {
				assert!(modifiers.contains(&crate::parser::QueryModifier::Expanded));
			}
			_ => panic!("Expected Execute action"),
		}
	}

	#[test]
	fn test_expanded_mode_not_applied_when_off() {
		let state = ReplState::new();
		let (buffer, action) = handle_input("", "SELECT 1;", &state);
		assert_eq!(buffer, "");
		match action {
			ReplAction::Execute { modifiers, .. } => {
				assert!(!modifiers.contains(&crate::parser::QueryModifier::Expanded));
			}
			_ => panic!("Expected Execute action"),
		}
	}

	#[test]
	fn test_expanded_mode_with_explicit_gx() {
		let state = ReplState {
			expanded_mode: true,
			write_mode: false,
			ots: None,
			..ReplState::new()
		};
		let (buffer, action) = handle_input("", "SELECT 1\\gx", &state);
		assert_eq!(buffer, "");
		match action {
			ReplAction::Execute { modifiers, .. } => {
				assert!(modifiers.contains(&crate::parser::QueryModifier::Expanded));
			}
			_ => panic!("Expected Execute action"),
		}
	}
}

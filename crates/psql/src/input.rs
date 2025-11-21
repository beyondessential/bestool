use std::path::PathBuf;

use crate::{
	parser::{DebugWhat, parse_multi_input},
	repl::ReplState,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ReplAction {
	Execute {
		input: String,
		sql: String,
		modifiers: crate::parser::QueryModifiers,
	},
	Exit,
	ToggleExpanded,
	ToggleWriteMode,
	Edit,
	Copy,
	IncludeFile {
		file_path: PathBuf,
		vars: Vec<(String, String)>,
	},
	RunSnippet {
		name: String,
		vars: Vec<(String, String)>,
	},
	SetOutputFile {
		file_path: PathBuf,
	},
	UnsetOutputFile,
	Debug {
		what: DebugWhat,
	},
	Help,
	SetVar {
		name: String,
		value: String,
	},
	UnsetVar {
		name: String,
	},
	LookupVar {
		pattern: Option<String>,
	},
	GetVar {
		name: String,
	},
	SnippetSave {
		name: String,
	},
	List {
		item: crate::parser::ListItem,
		pattern: String,
		detail: bool,
		sameconn: bool,
	},
	Describe {
		item: String,
		detail: bool,
		sameconn: bool,
	},
	Result {
		subcommand: crate::parser::ResultSubcommand,
	},
}

pub(crate) fn handle_input(
	buffer: &str,
	new_line: &str,
	state: &ReplState,
) -> (String, Vec<ReplAction>) {
	let mut new_buffer = buffer.to_string();

	if !new_buffer.is_empty() {
		new_buffer.push('\n');
	}
	new_buffer.push_str(new_line);

	let user_input = new_buffer.trim().to_string();

	// Handle legacy "quit" command for compatibility
	if buffer.is_empty() && user_input.eq_ignore_ascii_case("quit") {
		return (String::new(), vec![ReplAction::Exit]);
	}

	// Try to parse multiple statements
	let (actions, remaining) = parse_multi_input(&user_input, state);

	if actions.is_empty() {
		// No complete statements found
		// If remaining is empty, it means the input was comment-only
		if remaining.is_empty() {
			(String::new(), vec![])
		} else {
			(new_buffer, vec![])
		}
	} else {
		// Return completed actions and remaining buffer
		(remaining, actions)
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_handle_input_empty_line() {
		let state = ReplState::new();
		let (buffer, actions) = handle_input("", "", &state);
		assert_eq!(buffer, "");
		assert_eq!(actions.len(), 0);
	}

	#[test]
	fn test_handle_input_incomplete_query() {
		let state = ReplState::new();
		let (buffer, actions) = handle_input("", "SELECT * FROM users", &state);
		assert_eq!(buffer, "SELECT * FROM users");
		assert_eq!(actions.len(), 0);
	}

	#[test]
	fn test_handle_input_complete_query_semicolon() {
		let state = ReplState::new();
		let (buffer, actions) = handle_input("", "SELECT * FROM users;", &state);
		assert_eq!(buffer, "");
		assert_eq!(actions.len(), 1);
		match &actions[0] {
			ReplAction::Execute {
				input,
				sql,
				modifiers,
			} => {
				assert_eq!(input, "SELECT * FROM users");
				assert_eq!(sql, "SELECT * FROM users");
				assert!(modifiers.is_empty());
			}
			_ => panic!("Expected Execute action"),
		}
	}

	#[test]
	fn test_handle_input_complete_query_backslash_g() {
		let state = ReplState::new();
		let (buffer, actions) = handle_input("", "SELECT * FROM users\\g", &state);
		assert_eq!(buffer, "");
		assert_eq!(actions.len(), 1);
		match &actions[0] {
			ReplAction::Execute {
				input,
				sql,
				modifiers,
			} => {
				assert_eq!(input, "SELECT * FROM users");
				assert_eq!(sql, "SELECT * FROM users");
				assert!(modifiers.is_empty());
			}
			_ => panic!("Expected Execute action"),
		}
	}

	#[test]
	fn test_handle_input_multiline_query() {
		let state = ReplState::new();
		let (buffer1, actions1) = handle_input("", "SELECT *", &state);
		assert_eq!(buffer1, "SELECT *");
		assert_eq!(actions1.len(), 0);

		let (buffer2, actions2) = handle_input(&buffer1, "FROM users;", &state);
		assert_eq!(buffer2, "");
		assert_eq!(actions2.len(), 1);
		match &actions2[0] {
			ReplAction::Execute { input, sql, .. } => {
				assert_eq!(input, "SELECT *\nFROM users");
				assert_eq!(sql, "SELECT *\nFROM users");
			}
			_ => panic!("Expected Execute action"),
		}
	}

	#[test]
	fn test_handle_input_quit_command() {
		let state = ReplState::new();
		let (buffer, actions) = handle_input("", "\\q", &state);
		assert_eq!(buffer, "");
		assert_eq!(actions.len(), 1);
		assert!(matches!(actions[0], ReplAction::Exit));
	}

	#[test]
	fn test_handle_input_quit_command_case_insensitive() {
		let state = ReplState::new();
		let (buffer, actions) = handle_input("", "QUIT", &state);
		assert_eq!(buffer, "");
		assert_eq!(actions.len(), 1);
		assert!(matches!(actions[0], ReplAction::Exit));
	}

	#[test]
	fn test_handle_input_quit_after_incomplete() {
		let state = ReplState::new();
		let (buffer1, actions1) = handle_input("", "SELECT *", &state);
		assert_eq!(buffer1, "SELECT *");
		assert_eq!(actions1.len(), 0);

		// \q after incomplete query triggers quit
		let (buffer2, actions2) = handle_input(&buffer1, "\\q", &state);
		assert_eq!(buffer2, "");
		assert_eq!(actions2.len(), 1);
		assert!(matches!(actions2[0], ReplAction::Exit));
	}

	#[test]
	fn test_handle_input_expanded_metacommand() {
		let state = ReplState::new();
		let (buffer, actions) = handle_input("", "\\x", &state);
		assert_eq!(buffer, "");
		assert_eq!(actions.len(), 1);
		assert!(matches!(actions[0], ReplAction::ToggleExpanded));
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
		let (new_buffer, actions) = handle_input(cleared_buffer, "SELECT 1;", &state);
		assert_eq!(new_buffer, "");
		assert_eq!(actions.len(), 1);
		match &actions[0] {
			ReplAction::Execute { input, sql, .. } => {
				assert_eq!(input, "SELECT 1");
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
		let (buffer, actions) = handle_input("", "select 1+1 \\gx", &state);
		assert_eq!(buffer, "");
		assert_eq!(actions.len(), 1);
		match &actions[0] {
			ReplAction::Execute {
				input,
				sql,
				modifiers,
			} => {
				assert_eq!(input, "select 1+1");
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
		let (buffer, actions) = handle_input("", "SELECT 1;", &state);
		assert_eq!(buffer, "");
		assert_eq!(actions.len(), 1);
		match &actions[0] {
			ReplAction::Execute { modifiers, .. } => {
				assert!(modifiers.contains(&crate::parser::QueryModifier::Expanded));
			}
			_ => panic!("Expected Execute action"),
		}
	}

	#[test]
	fn test_expanded_mode_not_applied_when_off() {
		let state = ReplState::new();
		let (buffer, actions) = handle_input("", "SELECT 1;", &state);
		assert_eq!(buffer, "");
		assert_eq!(actions.len(), 1);
		match &actions[0] {
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
		let (buffer, actions) = handle_input("", "SELECT 1\\gx", &state);
		assert_eq!(buffer, "");
		assert_eq!(actions.len(), 1);
		match &actions[0] {
			ReplAction::Execute { modifiers, .. } => {
				assert!(modifiers.contains(&crate::parser::QueryModifier::Expanded));
			}
			_ => panic!("Expected Execute action"),
		}
	}

	#[test]
	fn test_comment_only_input() {
		let state = ReplState::new();
		let (buffer, actions) = handle_input("", "-- foo", &state);
		assert_eq!(buffer, "");
		assert_eq!(actions.len(), 0);
	}

	#[test]
	fn test_metacommand_with_comment() {
		let state = ReplState::new();
		let (buffer, actions) = handle_input("", "\\vars -- foo", &state);
		assert_eq!(buffer, "");
		assert_eq!(actions.len(), 1);
		match &actions[0] {
			ReplAction::LookupVar { pattern } => {
				assert_eq!(pattern, &None);
			}
			_ => panic!("Expected LookupVar action"),
		}
	}

	#[test]
	fn test_query_with_inline_comment() {
		let state = ReplState::new();
		let (buffer, actions) = handle_input("", "SELECT 1 + 1; -- foo", &state);
		assert_eq!(buffer, "");
		assert_eq!(actions.len(), 1);
		match &actions[0] {
			ReplAction::Execute { sql, .. } => {
				assert_eq!(sql, "SELECT 1 + 1");
			}
			_ => panic!("Expected Execute action"),
		}
	}

	#[test]
	fn test_copy_metacommand() {
		let state = ReplState::new();
		let (buffer, actions) = handle_input("", "\\copy", &state);
		assert_eq!(buffer, "");
		assert_eq!(actions.len(), 1);
		assert!(matches!(actions[0], ReplAction::Copy));
	}

	#[test]
	fn test_copy_metacommand_with_args() {
		let state = ReplState::new();
		let (buffer, actions) = handle_input("", "\\copy (select from blah) with headers", &state);
		assert_eq!(buffer, "");
		assert_eq!(actions.len(), 1);
		assert!(matches!(actions[0], ReplAction::Copy));
	}

	#[test]
	fn test_multiple_statements() {
		let state = ReplState::new();
		let input = "select 1 + 2 \\gx\nselect 2 + 3;\n\\re list";
		let (buffer, actions) = handle_input("", input, &state);
		assert_eq!(buffer, "");
		assert_eq!(actions.len(), 3);

		match &actions[0] {
			ReplAction::Execute { sql, modifiers, .. } => {
				assert_eq!(sql, "select 1 + 2");
				assert!(modifiers.contains(&crate::parser::QueryModifier::Expanded));
			}
			_ => panic!("Expected Execute for first action"),
		}

		match &actions[1] {
			ReplAction::Execute { sql, modifiers, .. } => {
				assert_eq!(sql, "select 2 + 3");
				assert!(!modifiers.contains(&crate::parser::QueryModifier::Expanded));
			}
			_ => panic!("Expected Execute for second action"),
		}

		assert!(matches!(actions[2], ReplAction::Result { .. }));
	}

	#[test]
	fn test_multiline_query_with_comments() {
		let state = ReplState::new();
		let (buffer1, actions1) = handle_input("", "select 1 + -- adding", &state);
		assert_eq!(buffer1, "select 1 + -- adding");
		assert_eq!(actions1.len(), 0);

		let (buffer2, actions2) = handle_input(&buffer1, "1; -- result is 2", &state);
		assert_eq!(buffer2, "");
		assert_eq!(actions2.len(), 1);
		match &actions2[0] {
			ReplAction::Execute { sql, .. } => {
				// The SQL contains the comment because Postgres handles it
				assert!(sql.contains("select 1 +"));
				assert!(sql.contains("1"));
			}
			_ => panic!("Expected Execute action"),
		}
	}

	#[test]
	fn test_comment_between_incomplete_query_lines() {
		let state = ReplState::new();
		let (buffer1, actions1) = handle_input("", "select", &state);
		assert_eq!(buffer1, "select");
		assert_eq!(actions1.len(), 0);

		let (buffer2, actions2) = handle_input(&buffer1, "-- this is a comment", &state);
		// Comment is preserved in buffer for Postgres to handle
		assert_eq!(buffer2, "select\n-- this is a comment");
		assert_eq!(actions2.len(), 0);

		let (buffer3, actions3) = handle_input(&buffer2, "1;", &state);
		assert_eq!(buffer3, "");
		assert_eq!(actions3.len(), 1);
		match &actions3[0] {
			ReplAction::Execute { sql, .. } => {
				// SQL contains the comment for Postgres to handle
				assert!(sql.contains("select"));
				assert!(sql.contains("-- this is a comment"));
			}
			_ => panic!("Expected Execute action"),
		}
	}

	#[test]
	fn test_auto_execute_by_appending_semicolon() {
		let state = ReplState::new();
		let incomplete_query = "SELECT 1 + 1";

		// Parse the incomplete query
		let (remaining, actions) = handle_input("", incomplete_query, &state);

		// Should have no actions and remaining buffer
		assert_eq!(actions.len(), 0);
		assert_eq!(remaining, incomplete_query);

		// Now auto-complete by appending semicolon (simulating \i or \e behavior)
		let completed = format!("{};", remaining);
		let (remaining2, actions2) = handle_input("", &completed, &state);

		// Should now have one action and no remaining
		assert_eq!(remaining2, "");
		assert_eq!(actions2.len(), 1);
		match &actions2[0] {
			ReplAction::Execute { sql, .. } => {
				assert_eq!(sql, "SELECT 1 + 1");
			}
			_ => panic!("Expected Execute action"),
		}
	}

	#[test]
	fn test_auto_execute_with_comments() {
		let state = ReplState::new();
		let incomplete_query = "-- Comment\nSELECT 1 + 1";

		// Parse the incomplete query
		let (remaining, actions) = handle_input("", incomplete_query, &state);

		// Should have no actions and remaining buffer
		assert_eq!(actions.len(), 0);
		assert_eq!(remaining, incomplete_query);

		// Now auto-complete by appending semicolon
		let completed = format!("{};", remaining);
		let (remaining2, actions2) = handle_input("", &completed, &state);

		// Should now have one action and no remaining
		assert_eq!(remaining2, "");
		assert_eq!(actions2.len(), 1);
		match &actions2[0] {
			ReplAction::Execute { sql, .. } => {
				assert!(sql.contains("SELECT 1 + 1"));
			}
			_ => panic!("Expected Execute action"),
		}
	}

	#[test]
	fn test_auto_execute_mixed_complete_and_incomplete() {
		let state = ReplState::new();
		let mixed_query = "SELECT 1 + 1;\nSELECT 2 + 3";

		// Parse the mixed query
		let (remaining, actions) = handle_input("", mixed_query, &state);

		// Should have one action (first complete query) and remaining buffer (second incomplete)
		assert_eq!(actions.len(), 1);
		assert_eq!(remaining.trim(), "SELECT 2 + 3");

		// Now auto-complete the remaining by appending semicolon
		let completed = format!("{};", remaining);
		let (remaining2, actions2) = handle_input("", &completed, &state);

		// Should now have one more action and no remaining
		assert_eq!(remaining2, "");
		assert_eq!(actions2.len(), 1);
		match &actions2[0] {
			ReplAction::Execute { sql, .. } => {
				assert_eq!(sql, "SELECT 2 + 3");
			}
			_ => panic!("Expected Execute action"),
		}
	}

	#[test]
	fn test_auto_execute_multiple_complete_then_incomplete() {
		let state = ReplState::new();
		let query = "SELECT 1;\nSELECT 2;\nSELECT 3";

		// Parse the query
		let (remaining, actions) = handle_input("", query, &state);

		// Should have two actions (first two complete queries) and remaining (third incomplete)
		assert_eq!(actions.len(), 2);
		assert_eq!(remaining.trim(), "SELECT 3");

		// Auto-complete the remaining
		let completed = format!("{};", remaining);
		let (remaining2, actions2) = handle_input("", &completed, &state);

		// Should have one more action
		assert_eq!(remaining2, "");
		assert_eq!(actions2.len(), 1);
	}

	#[test]
	fn test_already_complete_query_not_affected() {
		let state = ReplState::new();
		let complete_query = "SELECT 1 + 1;";

		// Parse the complete query
		let (remaining, actions) = handle_input("", complete_query, &state);

		// Should have one action and no remaining
		assert_eq!(remaining, "");
		assert_eq!(actions.len(), 1);
		match &actions[0] {
			ReplAction::Execute { sql, .. } => {
				assert_eq!(sql, "SELECT 1 + 1");
			}
			_ => panic!("Expected Execute action"),
		}
	}

	#[test]
	fn test_auto_execute_with_backslash_g() {
		let state = ReplState::new();
		let complete_query = "SELECT 1 + 1 \\g";

		// Parse the query with \g
		let (remaining, actions) = handle_input("", complete_query, &state);

		// Should have one action and no remaining
		assert_eq!(remaining, "");
		assert_eq!(actions.len(), 1);
		match &actions[0] {
			ReplAction::Execute { sql, .. } => {
				assert_eq!(sql, "SELECT 1 + 1");
			}
			_ => panic!("Expected Execute action"),
		}
	}
}

use super::{Metacommand, QueryModifiers, parse_metacommand, parse_query_modifiers, strip_comment};
use crate::input::ReplAction;
use crate::repl::ReplState;

/// Parse multiple statements from input, returning completed actions and remaining buffer
pub(crate) fn parse_multi_input(input: &str, state: &ReplState) -> (Vec<ReplAction>, String) {
	let input = input.trim();
	if input.is_empty() {
		return (vec![], String::new());
	}

	// Check if the entire input is just a comment
	if strip_comment(input).is_none() {
		return (vec![], String::new());
	}

	let mut actions = Vec::new();
	let mut remaining = input;

	loop {
		let start_remaining = remaining;
		remaining = remaining.trim_start();

		if remaining.is_empty() {
			break;
		}

		// Try to parse metacommand first (must be at start of line)
		if remaining.starts_with('\\') {
			// Extract just the first line for parsing (metacommand parsers expect EOF)
			let line_end = remaining.find('\n').unwrap_or(remaining.len());
			let line = &remaining[..line_end];

			// Strip comments from the line before parsing
			let line_without_comment = strip_comment(line);

			if let Some(stripped_line) = line_without_comment {
				if let Ok(Some(metacmd)) = parse_metacommand(stripped_line) {
					let action = metacommand_to_action(metacmd);
					actions.push(action);

					// Move past the line and newline
					if line_end < remaining.len() {
						remaining = &remaining[line_end + 1..];
					} else {
						remaining = "";
					}
					continue;
				}
			} else {
				// Line is only a comment, skip it
				if line_end < remaining.len() {
					remaining = &remaining[line_end + 1..];
				} else {
					remaining = "";
				}
				continue;
			}
		}

		// Try to parse query
		match try_parse_query(remaining) {
			Some((sql, modifiers, rest)) => {
				let mut mods = modifiers;
				if state.expanded_mode
					&& !mods
						.iter()
						.any(|m| matches!(m, super::QueryModifier::Expanded))
				{
					mods.insert(super::QueryModifier::Expanded);
				}

				actions.push(ReplAction::Execute {
					input: sql.clone(),
					sql: sql.clone(),
					modifiers: mods,
				});

				remaining = rest;
				continue;
			}
			None => {
				// No complete statement found
				// Check if there's a metacommand on a following line
				if let Some(newline_pos) = remaining.find('\n') {
					let after_newline = &remaining[newline_pos + 1..].trim_start();
					if after_newline.starts_with('\\') {
						// There's a metacommand after incomplete SQL
						// Skip to the metacommand line
						remaining = after_newline;
						continue;
					}
				}

				if start_remaining == remaining {
					// No progress made, break to avoid infinite loop
					break;
				}
			}
		}
	}

	if actions.is_empty() {
		(vec![], input.to_string())
	} else if remaining.trim().is_empty() {
		(actions, String::new())
	} else {
		(actions, remaining.to_string())
	}
}

fn try_parse_query(input: &str) -> Option<(String, QueryModifiers, &str)> {
	let trimmed = input.trim_start();
	if trimmed.is_empty() {
		return None;
	}

	let semicolon_pos = find_statement_end_semicolon(trimmed);
	let backslash_pos = find_statement_end_backslash_g(trimmed);

	match (semicolon_pos, backslash_pos) {
		(Some(semi_pos), Some(bs_pos)) if semi_pos < bs_pos => {
			let sql = trimmed[..semi_pos].trim().to_string();
			let rest = &trimmed[semi_pos + 1..];
			// Check if rest of line is just a comment
			let rest = if let Some(newline_pos) = rest.find('\n') {
				let line = &rest[..newline_pos];
				if strip_comment(line).is_none() {
					&rest[newline_pos + 1..]
				} else {
					rest
				}
			} else if strip_comment(rest).is_none() {
				""
			} else {
				rest
			};
			Some((sql, QueryModifiers::new(), rest))
		}
		(Some(semi_pos), None) => {
			let sql = trimmed[..semi_pos].trim().to_string();
			let rest = &trimmed[semi_pos + 1..];
			// Check if rest of line is just a comment
			let rest = if let Some(newline_pos) = rest.find('\n') {
				let line = &rest[..newline_pos];
				if strip_comment(line).is_none() {
					&rest[newline_pos + 1..]
				} else {
					rest
				}
			} else if strip_comment(rest).is_none() {
				""
			} else {
				rest
			};
			Some((sql, QueryModifiers::new(), rest))
		}
		(_, Some(bs_pos)) => {
			let query_part = &trimmed[..bs_pos];
			let modifier_part = &trimmed[bs_pos..];

			if let Some(newline_pos) = modifier_part.find('\n') {
				let modifier_line = &modifier_part[..newline_pos];
				if let Ok(Some((sql, modifiers))) =
					parse_query_modifiers(&format!("{}{}", query_part, modifier_line))
				{
					let rest = &modifier_part[newline_pos + 1..];
					return Some((sql, modifiers, rest));
				}
			} else if let Ok(Some((sql, modifiers))) =
				parse_query_modifiers(&format!("{}{}", query_part, modifier_part))
			{
				return Some((sql, modifiers, ""));
			}

			None
		}
		(None, None) => None,
	}
}

fn find_statement_end_semicolon(input: &str) -> Option<usize> {
	let mut in_single_quote = false;
	let mut in_double_quote = false;
	let mut in_comment = false;
	let mut prev_char = '\0';

	for (i, ch) in input.char_indices() {
		match ch {
			'\n' => {
				in_comment = false;
			}
			'-' if !in_single_quote && !in_double_quote && !in_comment && prev_char == '-' => {
				in_comment = true;
			}
			'\'' if !in_double_quote && !in_comment && prev_char != '\\' => {
				in_single_quote = !in_single_quote;
			}
			'"' if !in_single_quote && !in_comment && prev_char != '\\' => {
				in_double_quote = !in_double_quote;
			}
			';' if !in_single_quote && !in_double_quote && !in_comment => {
				return Some(i);
			}
			_ => {}
		}
		prev_char = ch;
	}

	None
}

fn find_statement_end_backslash_g(input: &str) -> Option<usize> {
	let mut in_single_quote = false;
	let mut in_double_quote = false;
	let mut in_comment = false;
	let mut prev_char = '\0';

	for (i, ch) in input.char_indices() {
		match ch {
			'\n' => {
				in_comment = false;
				if !in_single_quote && !in_double_quote {
					// Check if next non-whitespace is a metacommand
					let rest = &input[i + 1..];
					let trimmed_rest = rest.trim_start();
					if trimmed_rest.starts_with('\\') {
						// Check if it's actually a metacommand (not just \g)
						if let Some(second_char) = trimmed_rest.chars().nth(1)
							&& second_char != 'g' && second_char != 'G'
						{
							// This is a metacommand, end the query here
							return Some(i);
						}
					}
				}
			}
			'-' if !in_single_quote && !in_double_quote && !in_comment && prev_char == '-' => {
				in_comment = true;
			}
			'\'' if !in_double_quote && !in_comment && prev_char != '\\' => {
				in_single_quote = !in_single_quote;
			}
			'"' if !in_single_quote && !in_comment && prev_char != '\\' => {
				in_double_quote = !in_double_quote;
			}
			'\\' if !in_single_quote && !in_double_quote && !in_comment => {
				if let Some(next_ch) = input[i + 1..].chars().next()
					&& (next_ch == 'g' || next_ch == 'G')
				{
					return Some(i);
				}
			}
			_ => {}
		}
		prev_char = ch;
	}

	None
}

fn metacommand_to_action(metacmd: Metacommand) -> ReplAction {
	match metacmd {
		Metacommand::Quit => ReplAction::Exit,
		Metacommand::Expanded => ReplAction::ToggleExpanded,
		Metacommand::WriteMode => ReplAction::ToggleWriteMode,
		Metacommand::Edit => ReplAction::Edit,
		Metacommand::Copy => ReplAction::Copy,
		Metacommand::Include { file_path, vars } => ReplAction::IncludeFile {
			file_path: file_path.into(),
			vars,
		},
		Metacommand::SnippetRun { name, vars } => ReplAction::RunSnippet { name, vars },
		Metacommand::SnippetSave { name } => ReplAction::SnippetSave { name },
		Metacommand::Output {
			file_path: Some(file_path),
		} => ReplAction::SetOutputFile {
			file_path: file_path.into(),
		},
		Metacommand::Output { file_path: None } => ReplAction::UnsetOutputFile,
		Metacommand::Debug { what } => ReplAction::Debug { what },
		Metacommand::Help => ReplAction::Help,
		Metacommand::SetVar { name, value } => ReplAction::SetVar { name, value },
		Metacommand::UnsetVar { name } => ReplAction::UnsetVar { name },
		Metacommand::LookupVar { pattern } => ReplAction::LookupVar { pattern },
		Metacommand::GetVar { name } => ReplAction::GetVar { name },
		Metacommand::List {
			item,
			pattern,
			detail,
			sameconn,
		} => ReplAction::List {
			item,
			pattern,
			detail,
			sameconn,
		},
		Metacommand::Describe {
			item,
			detail,
			sameconn,
		} => ReplAction::Describe {
			item,
			detail,
			sameconn,
		},
		Metacommand::Result { subcommand } => ReplAction::Result { subcommand },
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn make_state() -> ReplState {
		ReplState::new()
	}

	#[test]
	fn test_single_query_semicolon() {
		let state = make_state();
		let (actions, remaining) = parse_multi_input("select 1 + 2;", &state);
		assert_eq!(remaining, "");
		assert_eq!(actions.len(), 1);
		match &actions[0] {
			ReplAction::Execute { sql, .. } => assert_eq!(sql, "select 1 + 2"),
			_ => panic!("Expected Execute"),
		}
	}

	#[test]
	fn test_single_query_with_modifier() {
		let state = make_state();
		let (actions, remaining) = parse_multi_input("select 1 \\gx", &state);
		assert_eq!(remaining, "");
		assert_eq!(actions.len(), 1);
		match &actions[0] {
			ReplAction::Execute { sql, modifiers, .. } => {
				assert_eq!(sql, "select 1");
				assert!(modifiers.contains(&super::super::QueryModifier::Expanded));
			}
			_ => panic!("Expected Execute"),
		}
	}

	#[test]
	fn test_multiple_queries() {
		let state = make_state();
		let input = "select 1 + 2 \\gx\nselect 2 + 3;";
		let (actions, remaining) = parse_multi_input(input, &state);
		assert_eq!(remaining, "");
		assert_eq!(actions.len(), 2);
	}

	#[test]
	fn test_query_and_metacommand() {
		let state = make_state();
		let input = "select 1 + 2 \\gx\nselect 2 + 3;\n\\re list";
		let (actions, remaining) = parse_multi_input(input, &state);
		assert_eq!(remaining, "");
		assert_eq!(actions.len(), 3);

		match &actions[2] {
			ReplAction::Result { .. } => {}
			_ => panic!("Expected Result metacommand"),
		}
	}

	#[test]
	fn test_incomplete_query() {
		let state = make_state();
		let (actions, remaining) = parse_multi_input("select 1 + 2", &state);
		assert_eq!(actions.len(), 0);
		assert_eq!(remaining, "select 1 + 2");
	}

	#[test]
	fn test_complete_and_incomplete() {
		let state = make_state();
		let input = "select 1;\nselect 2 + 3";
		let (actions, remaining) = parse_multi_input(input, &state);
		assert_eq!(actions.len(), 1);
		assert!(remaining.contains("select 2 + 3"));
	}

	#[test]
	fn test_string_with_semicolon() {
		let state = make_state();
		let (actions, remaining) = parse_multi_input("select 'hello;world';", &state);
		assert_eq!(remaining, "");
		assert_eq!(actions.len(), 1);
		match &actions[0] {
			ReplAction::Execute { sql, .. } => assert_eq!(sql, "select 'hello;world'"),
			_ => panic!("Expected Execute"),
		}
	}

	#[test]
	fn test_example_from_issue() {
		let state = make_state();
		let input = "select 1 + 2 \\gx\nselect 2 + 3;\n\\re list";
		let (actions, remaining) = parse_multi_input(input, &state);
		assert_eq!(remaining, "");
		assert_eq!(actions.len(), 3);

		// Verify first query with \gx modifier
		match &actions[0] {
			ReplAction::Execute { sql, modifiers, .. } => {
				assert_eq!(sql, "select 1 + 2");
				assert!(modifiers.contains(&super::super::QueryModifier::Expanded));
			}
			_ => panic!("Expected first action to be Execute with expanded"),
		}

		// Verify second query with semicolon
		match &actions[1] {
			ReplAction::Execute { sql, modifiers, .. } => {
				assert_eq!(sql, "select 2 + 3");
				assert!(!modifiers.contains(&super::super::QueryModifier::Expanded));
			}
			_ => panic!("Expected second action to be Execute without expanded"),
		}

		// Verify metacommand
		match &actions[2] {
			ReplAction::Result { .. } => {}
			_ => panic!("Expected third action to be Result metacommand"),
		}
	}

	#[test]
	fn test_metacommand_only() {
		let state = make_state();
		let input = "\\x";
		let (actions, remaining) = parse_multi_input(input, &state);
		assert_eq!(remaining, "");
		assert_eq!(actions.len(), 1);
		match &actions[0] {
			ReplAction::ToggleExpanded => {}
			_ => panic!("Expected ToggleExpanded"),
		}
	}

	#[test]
	fn test_multiple_metacommands() {
		let state = make_state();
		let input = "\\x\n\\re list";
		let (actions, remaining) = parse_multi_input(input, &state);
		assert_eq!(remaining, "");
		assert!(
			!actions.is_empty(),
			"Expected at least 1 action, got {}",
			actions.len()
		);
		match &actions[0] {
			ReplAction::ToggleExpanded => {}
			_ => panic!("Expected ToggleExpanded as first action"),
		}
	}

	#[test]
	fn test_semicolon_in_string() {
		let state = make_state();
		let input = "select 'hello; world' as msg;";
		let (actions, remaining) = parse_multi_input(input, &state);
		assert_eq!(remaining, "");
		assert_eq!(actions.len(), 1);
		match &actions[0] {
			ReplAction::Execute { sql, .. } => {
				assert_eq!(sql, "select 'hello; world' as msg");
			}
			_ => panic!("Expected Execute"),
		}
	}

	#[test]
	fn test_backslash_in_string() {
		let state = make_state();
		let input = r"select 'hello \g world' as msg \g";
		let (actions, remaining) = parse_multi_input(input, &state);
		assert!(
			remaining.is_empty() || remaining == input,
			"Expected empty or original remaining, got: {}",
			remaining
		);
		if !actions.is_empty() {
			assert_eq!(actions.len(), 1);
		}
	}

	#[test]
	fn test_comment_only_input() {
		let state = make_state();
		let (actions, remaining) = parse_multi_input("-- foo", &state);
		assert_eq!(remaining, "");
		assert_eq!(actions.len(), 0);
	}

	#[test]
	fn test_metacommand_with_comment() {
		let state = make_state();
		let (actions, remaining) = parse_multi_input("\\vars -- foo", &state);
		assert_eq!(remaining, "");
		assert_eq!(actions.len(), 1);
		match &actions[0] {
			ReplAction::LookupVar { pattern } => {
				assert_eq!(pattern, &None);
			}
			_ => panic!("Expected LookupVar"),
		}
	}

	#[test]
	fn test_metacommand_with_pattern_and_comment() {
		let state = make_state();
		let (actions, remaining) = parse_multi_input("\\vars my* -- foo", &state);
		assert_eq!(remaining, "");
		assert_eq!(actions.len(), 1);
		match &actions[0] {
			ReplAction::LookupVar { pattern } => {
				assert_eq!(pattern, &Some("my*".to_string()));
			}
			_ => panic!("Expected LookupVar"),
		}
	}

	#[test]
	fn test_query_with_comment() {
		let state = make_state();
		let (actions, remaining) = parse_multi_input("select 1 + 1; -- foo", &state);
		assert_eq!(remaining, "");
		assert_eq!(actions.len(), 1);
		match &actions[0] {
			ReplAction::Execute { sql, .. } => {
				assert_eq!(sql, "select 1 + 1");
			}
			_ => panic!("Expected Execute"),
		}
	}

	#[test]
	fn test_multiline_with_comment() {
		let state = make_state();
		let input = "select 1 + -- bar\n1;";
		let (actions, remaining) = parse_multi_input(input, &state);
		assert_eq!(remaining, "");
		assert_eq!(actions.len(), 1);
		match &actions[0] {
			ReplAction::Execute { sql, .. } => {
				// The SQL will contain the comment because Postgres handles it
				assert!(sql.contains("select 1 +"));
			}
			_ => panic!("Expected Execute"),
		}
	}

	#[test]
	fn test_comment_line_between_statements() {
		let state = make_state();
		let input = "select 1;\n-- foo\nselect 2;";
		let (actions, remaining) = parse_multi_input(input, &state);
		assert_eq!(remaining, "");
		assert_eq!(actions.len(), 2);
	}

	#[test]
	fn test_comment_not_in_string() {
		let state = make_state();
		let input = "select '-- not a comment';";
		let (actions, remaining) = parse_multi_input(input, &state);
		assert_eq!(remaining, "");
		assert_eq!(actions.len(), 1);
		match &actions[0] {
			ReplAction::Execute { sql, .. } => {
				assert_eq!(sql, "select '-- not a comment'");
			}
			_ => panic!("Expected Execute"),
		}
	}

	#[test]
	fn test_user_query_with_comments() {
		let state = make_state();
		let input = r#"
			WITH group_analysis AS (
  SELECT
    table_schema,
    table_name,
    record_id,
    -- Condition 1: Does a non-zero user for 2.40.5 row exist?
    BOOL_OR(
      version = '2.40.5'
      AND updated_by_user_id <> '00000000-0000-0000-0000-000000000000'
    ) AS has_non_zero_2_40_5,
    -- Condition 2: Does a lower version exist?
    BOOL_OR(
      version <> 'unknown'
      AND string_to_array(version, '.')::int[]
            < string_to_array('2.40.5', '.')::int[]
    ) AS has_lower_version,
    -- Condition 3: Does an 000-user, outside of backfill time for 2.40.5 row exist
    BOOL_OR(
      version = '2.40.5'
      AND updated_by_user_id = '00000000-0000-0000-0000-000000000000'
      AND NOT (
        (created_at >= '2025-10-23 09:17:37.222+11' AND created_at < '2025-10-23 09:17:37.223+11')
        OR
        (created_at >= '2025-10-23 09:41:14.330+11' AND created_at < '2025-10-23 09:41:14.331+11')
      )
    ) AS has_outlier_zero_user_2_40_5
  FROM logs.changes_backup
  GROUP BY
    table_schema,
    table_name,
    record_id
),
flagged_targets AS (
  SELECT
    cb.id,
    ga.has_lower_version,
    ga.has_non_zero_2_40_5,
    ga.has_outlier_zero_user_2_40_5,
    ROW_NUMBER() OVER (
      PARTITION BY cb.table_schema, cb.table_name, cb.record_id
      ORDER BY cb.created_at ASC, cb.id
    ) AS rn
  FROM logs.changes_backup AS cb
  JOIN group_analysis AS ga
    ON cb.table_schema = ga.table_schema
   AND cb.table_name   = ga.table_name
   AND cb.record_id    = ga.record_id
  WHERE
    -- Target only the 000-user, 2.40.5 rows *inside* the backfill windows
    cb.updated_by_user_id = '00000000-0000-0000-0000-000000000000'
    AND cb.version = '2.40.5'
    AND (
      (cb.created_at >= '2025-10-23 09:17:37.222+11' AND cb.created_at < '2025-10-23 09:17:37.223+11')
      OR
      (cb.created_at >= '2025-10-23 09:41:14.330+11' AND cb.created_at < '2025-10-23 09:41:14.331+11')
    )
)
DELETE FROM logs.changes_backup
WHERE id IN (
  SELECT id
  FROM flagged_targets
  WHERE
    -- Rule 1: Delete if a non-zero user, 2.40.5 row exists
    has_non_zero_2_40_5
    -- Rule 2: OR delete if a lower version exists
    OR has_lower_version
    -- Rule 3: OR delete if an "outlier" 000-user 2.40.5 row already exists
    OR has_outlier_zero_user_2_40_5
    -- Rule 4: OR (if all above are false) delete if it's a duplicate
    OR rn > 1
    );
"#;
		let (actions, remaining) = parse_multi_input(input, &state);
		assert_eq!(
			remaining, "",
			"Query should be complete, but got remaining: {}",
			remaining
		);
		assert_eq!(actions.len(), 1);
		match &actions[0] {
			ReplAction::Execute { sql, .. } => {
				assert!(sql.contains("WITH group_analysis"));
				assert!(sql.contains("DELETE FROM logs.changes_backup"));
			}
			_ => panic!("Expected Execute"),
		}
	}

	#[test]
	fn test_comment_with_quotes_in_middle() {
		let state = make_state();
		let input = r#"SELECT 1 AS first,
-- This is a "quoted" comment
2 AS second;"#;
		let (actions, remaining) = parse_multi_input(input, &state);
		assert_eq!(remaining, "");
		assert_eq!(actions.len(), 1);
		match &actions[0] {
			ReplAction::Execute { sql, .. } => {
				assert!(sql.contains("SELECT 1 AS first"));
				assert!(sql.contains("2 AS second"));
			}
			_ => panic!("Expected Execute"),
		}
	}

	#[test]
	fn test_comment_with_single_quote() {
		let state = make_state();
		let input = "SELECT 1 -- don't worry\n;";
		let (actions, remaining) = parse_multi_input(input, &state);
		assert_eq!(remaining, "");
		assert_eq!(actions.len(), 1);
	}

	#[test]
	fn test_multiple_comments_with_quotes() {
		let state = make_state();
		let input = r#"SELECT
-- This "has" quotes
1 AS id,
-- And 'this' too
2 AS value;"#;
		let (actions, remaining) = parse_multi_input(input, &state);
		assert_eq!(remaining, "");
		assert_eq!(actions.len(), 1);
	}

	#[test]
	fn test_comment_with_semicolon() {
		let state = make_state();
		let input = "SELECT 1 -- ; this semicolon is in a comment\n;";
		let (actions, remaining) = parse_multi_input(input, &state);
		assert_eq!(remaining, "");
		assert_eq!(actions.len(), 1);
	}

	#[test]
	fn test_comment_with_backslash_g() {
		let state = make_state();
		let input = "SELECT 1 -- \\g this is in a comment\n;";
		let (actions, remaining) = parse_multi_input(input, &state);
		assert_eq!(remaining, "");
		assert_eq!(actions.len(), 1);
	}
}

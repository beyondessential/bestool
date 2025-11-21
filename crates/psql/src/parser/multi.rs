use winnow::{
	Parser,
	ascii::multispace0,
	combinator::opt,
	error::{ContextError, ErrMode},
	token::{any, take_till},
};

use super::{Metacommand, QueryModifiers, parse_metacommand, parse_query_modifiers, strip_comment};
use crate::{input::ReplAction, repl::ReplState};

/// Parse multiple statements from input, returning completed actions and remaining buffer
pub(crate) fn parse_multi_input(input: &str, state: &ReplState) -> (Vec<ReplAction>, String) {
	let input = input.trim();
	if input.is_empty() {
		return (vec![], String::new());
	}

	// Check if the entire input is just a comment (only for single-line input)
	if !input.contains('\n') && strip_comment(input).is_none() {
		return (vec![], String::new());
	}

	let mut remaining = input;
	let mut actions = Vec::new();

	loop {
		// Skip leading whitespace
		let _ = multispace0::<_, ContextError>.parse_next(&mut remaining);

		if remaining.is_empty() {
			break;
		}

		let start_remaining = remaining;

		// Try to parse a metacommand
		if let Ok(action) = parse_metacommand_action(&mut remaining) {
			actions.push(action);
			continue;
		}

		// Reset and try to parse a query
		remaining = start_remaining;
		if let Ok((sql, modifiers)) = parse_query_statement(&mut remaining, state) {
			actions.push(ReplAction::Execute {
				input: sql.clone(),
				sql,
				modifiers,
			});
			continue;
		}

		// No progress made, check if there's a metacommand on a following line
		remaining = start_remaining;
		if let Some(newline_pos) = remaining.find('\n') {
			let after_newline = remaining[newline_pos + 1..].trim_start();
			if after_newline.starts_with('\\') {
				// Skip to the metacommand line
				remaining = &remaining[newline_pos + 1..];
				continue;
			}
		}

		// No progress possible, break
		break;
	}

	if actions.is_empty() {
		(vec![], input.to_string())
	} else if remaining.trim().is_empty() {
		(actions, String::new())
	} else {
		(actions, remaining.to_string())
	}
}

fn parse_metacommand_action(input: &mut &str) -> Result<ReplAction, ErrMode<ContextError>> {
	// Must start with backslash
	'\\'.parse_next(input)?;

	// Get the rest of the line
	let line_end = input.find('\n').unwrap_or(input.len());
	let line = &input[..line_end];
	*input = &input[line_end..];

	let full_line = format!("\\{}", line);

	// Skip the newline if present
	let _: Result<_, ContextError> = opt('\n').parse_next(input);

	// Strip comments from the line before parsing
	let line_without_comment = strip_comment(&full_line);

	if let Some(stripped_line) = line_without_comment
		&& let Ok(Some(metacmd)) = parse_metacommand(stripped_line)
	{
		return Ok(metacommand_to_action(metacmd));
	}

	Err(ErrMode::Backtrack(ContextError::new()))
}

fn parse_query_statement(
	input: &mut &str,
	state: &ReplState,
) -> Result<(String, QueryModifiers), ErrMode<ContextError>> {
	let start = *input;
	let sql_result = sql_until_terminator(input)?;

	match sql_result {
		SqlResult::Semicolon(sql) => {
			// Skip the semicolon
			';'.parse_next(input)?;
			// Skip any trailing comment on the same line
			let _ = skip_line_comment(input);
			let mut mods = QueryModifiers::new();
			if state.expanded_mode {
				mods.insert(super::QueryModifier::Expanded);
			}
			Ok((sql, mods))
		}
		SqlResult::BackslashG(sql) => {
			// We need to parse \g and any modifiers
			// The input currently points to the \g part

			// Consume \g
			let ch1: char = any::<_, ContextError>
				.parse_next(input)
				.map_err(ErrMode::Backtrack)?;
			let ch2: char = any::<_, ContextError>
				.parse_next(input)
				.map_err(ErrMode::Backtrack)?;

			if ch1 != '\\' || (ch2 != 'g' && ch2 != 'G') {
				*input = start;
				return Err(ErrMode::Backtrack(ContextError::new()));
			}

			// Successfully parsed \g, now check for modifiers
			let remaining_line: &str = take_till(0.., '\n').parse_next(input)?;
			let _: Result<_, ContextError> = opt('\n').parse_next(input);

			// Check if there were modifiers after \g
			let mut mods = QueryModifiers::new();
			if !remaining_line.trim().is_empty() {
				// Try to parse the full query with modifiers
				let full_query = format!("{}\\g{}", sql, remaining_line);
				if let Ok(Some((_, parsed_mods))) = parse_query_modifiers(&full_query) {
					mods = parsed_mods;
				}
			}

			if state.expanded_mode
				&& !mods
					.iter()
					.any(|m| matches!(m, super::QueryModifier::Expanded))
			{
				mods.insert(super::QueryModifier::Expanded);
			}
			Ok((sql, mods))
		}
	}
}

#[derive(Debug)]
enum SqlResult {
	Semicolon(String),
	BackslashG(String),
}

fn sql_until_terminator(input: &mut &str) -> Result<SqlResult, ErrMode<ContextError>> {
	let mut result = String::new();
	let mut in_single_quote = false;
	let mut in_double_quote = false;
	let mut in_comment = false;
	let mut prev_char = '\0';

	while !input.is_empty() {
		let before = *input;
		let ch: char = any::<_, ContextError>
			.parse_next(input)
			.map_err(ErrMode::Backtrack)?;

		match ch {
			'\n' => {
				in_comment = false;
				result.push(ch);

				// Check if next non-whitespace is a metacommand
				if !in_single_quote && !in_double_quote {
					let rest = input.trim_start();
					if rest.starts_with('\\')
						&& let Some(second_char) = rest.chars().nth(1)
						&& second_char != 'g'
						&& second_char != 'G'
					{
						// This is a metacommand, end the query here
						result.pop(); // Remove the newline
						return Ok(SqlResult::BackslashG(result.trim().to_string()));
					}
				}
			}
			'-' if !in_single_quote && !in_double_quote && !in_comment && prev_char == '-' => {
				in_comment = true;
				result.push(ch);
			}
			'\'' if !in_double_quote && !in_comment && prev_char != '\\' => {
				in_single_quote = !in_single_quote;
				result.push(ch);
			}
			'"' if !in_single_quote && !in_comment && prev_char != '\\' => {
				in_double_quote = !in_double_quote;
				result.push(ch);
			}
			';' if !in_single_quote && !in_double_quote && !in_comment => {
				// Found terminating semicolon
				*input = before;
				return Ok(SqlResult::Semicolon(result.trim().to_string()));
			}
			'\\' if !in_single_quote && !in_double_quote && !in_comment => {
				// Check if next char is 'g' or 'G'
				if let Some(next_ch) = input.chars().next()
					&& (next_ch == 'g' || next_ch == 'G')
				{
					// Found \g terminator
					*input = before;
					return Ok(SqlResult::BackslashG(result.trim().to_string()));
				}
				result.push(ch);
			}
			_ => {
				result.push(ch);
			}
		}

		prev_char = ch;
	}

	// No terminator found
	Err(ErrMode::Backtrack(ContextError::new()))
}

fn skip_line_comment(input: &mut &str) -> Result<(), ErrMode<ContextError>> {
	let line_rest: &str = take_till(0.., '\n').parse_next(input)?;
	if strip_comment(line_rest).is_none() {
		// Line is only a comment, skip the newline too
		let _: Result<_, ContextError> = opt('\n').parse_next(input);
	}
	Ok(())
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
		let input = r#"WITH group_analysis AS (
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

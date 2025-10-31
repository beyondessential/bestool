pub(super) fn should_exclude_system_schemas(pattern: &str) -> bool {
	// If the pattern explicitly mentions information_schema or pg_toast, don't exclude them
	if let Some((schema, _)) = pattern.split_once('.') {
		// Check if schema part explicitly matches these schemas
		schema != "information_schema" && schema != "pg_toast"
	} else {
		// Single word pattern (matches table in public) - exclude system schemas
		pattern != "information_schema" && pattern != "pg_toast"
	}
}

pub(super) fn parse_pattern(pattern: &str) -> (String, String) {
	if let Some((schema, table)) = pattern.split_once('.') {
		let schema_regex = wildcard_to_regex(schema);
		let table_regex = wildcard_to_regex(table);
		(schema_regex, table_regex)
	} else if pattern == "*" {
		// Special case: * matches all tables in all schemas
		(".*".to_string(), ".*".to_string())
	} else {
		// Pattern without dot matches table name in public schema
		("^public$".to_string(), wildcard_to_regex(pattern))
	}
}

fn wildcard_to_regex(pattern: &str) -> String {
	if pattern == "*" {
		return ".*".to_string();
	}

	let mut regex = String::from("^");
	for ch in pattern.chars() {
		match ch {
			'*' => regex.push_str(".*"),
			'?' => regex.push('.'),
			'.' | '+' | '(' | ')' | '[' | ']' | '{' | '}' | '^' | '$' | '|' | '\\' => {
				regex.push('\\');
				regex.push(ch);
			}
			_ => regex.push(ch),
		}
	}
	regex.push('$');
	regex
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_parse_pattern_with_schema_and_table() {
		let (schema, table) = parse_pattern("public.users");
		assert_eq!(schema, "^public$");
		assert_eq!(table, "^users$");
	}

	#[test]
	fn test_parse_pattern_with_wildcard_schema() {
		let (schema, table) = parse_pattern("public.*");
		assert_eq!(schema, "^public$");
		assert_eq!(table, ".*");
	}

	#[test]
	fn test_parse_pattern_with_wildcard_table() {
		let (schema, table) = parse_pattern("public.user*");
		assert_eq!(schema, "^public$");
		assert_eq!(table, "^user.*$");
	}

	#[test]
	fn test_parse_pattern_without_dot() {
		let (schema, table) = parse_pattern("users");
		assert_eq!(schema, "^public$");
		assert_eq!(table, "^users$");
	}

	#[test]
	fn test_parse_pattern_star_only() {
		let (schema, table) = parse_pattern("*");
		assert_eq!(schema, ".*");
		assert_eq!(table, ".*");
	}

	#[test]
	fn test_parse_pattern_star_dot_star() {
		let (schema, table) = parse_pattern("*.*");
		assert_eq!(schema, ".*");
		assert_eq!(table, ".*");
	}

	#[test]
	fn test_wildcard_to_regex_star() {
		assert_eq!(wildcard_to_regex("*"), ".*");
	}

	#[test]
	fn test_wildcard_to_regex_literal() {
		assert_eq!(wildcard_to_regex("users"), "^users$");
	}

	#[test]
	fn test_wildcard_to_regex_with_star() {
		assert_eq!(wildcard_to_regex("user*"), "^user.*$");
	}

	#[test]
	fn test_wildcard_to_regex_with_question() {
		assert_eq!(wildcard_to_regex("user?"), "^user.$");
	}

	#[test]
	fn test_wildcard_to_regex_escapes_special_chars() {
		assert_eq!(wildcard_to_regex("test.table"), "^test\\.table$");
	}

	#[test]
	fn test_should_exclude_system_schemas_default() {
		assert!(should_exclude_system_schemas("public.*"));
		assert!(should_exclude_system_schemas("*.*"));
		assert!(should_exclude_system_schemas("users"));
	}

	#[test]
	fn test_should_exclude_system_schemas_explicit() {
		assert!(!should_exclude_system_schemas("information_schema.*"));
		assert!(!should_exclude_system_schemas("pg_toast.*"));
		assert!(!should_exclude_system_schemas("information_schema"));
		assert!(!should_exclude_system_schemas("pg_toast"));
	}
}

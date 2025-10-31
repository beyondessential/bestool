use miette::Result;

/// Interpolate variables in the SQL string.
/// Replaces ${name} with the value of variable `name`.
/// Escape sequences: ${{name}} becomes ${name} (without replacement).
/// Returns error if a variable is referenced but not set.
pub fn interpolate_variables(
	sql: &str,
	vars: &std::collections::BTreeMap<String, String>,
) -> Result<String> {
	let bytes = sql.as_bytes();
	let mut result = String::new();
	let mut i = 0;

	while i < bytes.len() {
		if bytes[i] == b'$' && i + 1 < bytes.len() && bytes[i + 1] == b'{' {
			// Check if it's an escape sequence ${{
			if i + 2 < bytes.len() && bytes[i + 2] == b'{' {
				// Escape sequence: ${{ -> find }} and output ${...}
				i += 3; // skip ${
				result.push_str("${");

				// Find the closing }}
				while i < bytes.len() {
					if i + 1 < bytes.len() && bytes[i] == b'}' && bytes[i + 1] == b'}' {
						result.push('}');
						i += 2;
						break;
					}
					result.push(bytes[i] as char);
					i += 1;
				}
			} else {
				// Normal substitution: ${name}
				i += 2; // skip ${
				let var_start = i;

				// Find the closing }
				while i < bytes.len() && bytes[i] != b'}' {
					i += 1;
				}

				if i < bytes.len() && bytes[i] == b'}' {
					let var_name = std::str::from_utf8(&bytes[var_start..i])
						.unwrap_or_default()
						.trim();
					if let Some(value) = vars.get(var_name) {
						result.push_str(value);
					} else {
						miette::bail!("Variable '{}' is not set", var_name);
					}
					i += 1; // skip closing }
				}
			}
		} else {
			result.push(bytes[i] as char);
			i += 1;
		}
	}

	Ok(result)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_interpolate_variables_basic() {
		let mut vars = std::collections::BTreeMap::new();
		vars.insert("name".to_string(), "Alice".to_string());
		vars.insert("value".to_string(), "42".to_string());

		let sql = "SELECT * WHERE name = ${name} AND value = ${value}";
		let result = interpolate_variables(sql, &vars).unwrap();
		assert_eq!(result, "SELECT * WHERE name = Alice AND value = 42");
	}

	#[test]
	fn test_interpolate_variables_no_substitution() {
		let vars = std::collections::BTreeMap::new();
		let sql = "SELECT * FROM users";
		let result = interpolate_variables(sql, &vars).unwrap();
		assert_eq!(result, sql);
	}

	#[test]
	fn test_interpolate_variables_missing_var() {
		let vars = std::collections::BTreeMap::new();
		let sql = "SELECT * WHERE name = ${name}";
		let result = interpolate_variables(sql, &vars);
		assert!(result.is_err());
	}

	#[test]
	fn test_interpolate_variables_escape_sequence() {
		let mut vars = std::collections::BTreeMap::new();
		vars.insert("name".to_string(), "Alice".to_string());

		let sql = "SELECT ${{name}}, ${name}";
		let result = interpolate_variables(sql, &vars).unwrap();
		assert_eq!(result, "SELECT ${name}, Alice");
	}

	#[test]
	fn test_interpolate_variables_in_quoted_string() {
		let mut vars = std::collections::BTreeMap::new();
		vars.insert("name".to_string(), "O'Brien".to_string());

		let sql = "SELECT * WHERE name = '${name}'";
		let result = interpolate_variables(sql, &vars).unwrap();
		assert_eq!(result, "SELECT * WHERE name = 'O'Brien'");
	}

	#[test]
	fn test_interpolate_variables_multiple_escapes() {
		let mut vars = std::collections::BTreeMap::new();
		vars.insert("x".to_string(), "10".to_string());

		let sql = "SELECT ${{x}}, ${{x}}, ${x}";
		let result = interpolate_variables(sql, &vars).unwrap();
		assert_eq!(result, "SELECT ${x}, ${x}, 10");
	}
}

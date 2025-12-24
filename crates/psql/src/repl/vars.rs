use std::ops::ControlFlow;

use comfy_table::Table;

use super::state::ReplContext;

pub fn handle_set_var(ctx: &mut ReplContext<'_>, name: String, value: String) -> ControlFlow<()> {
	let mut state = ctx.repl_state.lock().unwrap();
	state.vars.insert(name, value);
	ControlFlow::Continue(())
}

pub fn handle_default_var(ctx: &mut ReplContext<'_>, name: String, value: String) -> ControlFlow<()> {
	let mut state = ctx.repl_state.lock().unwrap();
	state.vars.entry(name).or_insert(value);
	ControlFlow::Continue(())
}

pub fn handle_unset_var(ctx: &mut ReplContext<'_>, name: String) -> ControlFlow<()> {
	let mut state = ctx.repl_state.lock().unwrap();
	if state.vars.remove(&name).is_none() {
		eprintln!("Variable '{}' not found", name);
	}
	ControlFlow::Continue(())
}

pub fn handle_lookup_var(ctx: &mut ReplContext<'_>, pattern: Option<String>) -> ControlFlow<()> {
	let state = ctx.repl_state.lock().unwrap();

	let matching_vars: Vec<(&String, &String)> = if let Some(ref pat) = pattern {
		state
			.vars
			.iter()
			.filter(|(name, _)| matches_pattern(name, pat))
			.collect()
	} else {
		state.vars.iter().collect()
	};

	if matching_vars.is_empty() {
		if pattern.is_some() {
			eprintln!("No variables match the pattern");
		} else {
			eprintln!("No variables defined");
		}
		return ControlFlow::Continue(());
	}

	let mut table = Table::new();
	crate::table::configure(&mut table);
	table.set_header(vec!["Name", "Value"]);
	crate::table::style_header(&mut table);

	for (name, value) in matching_vars {
		table.add_row(vec![name, value]);
	}

	println!("{table}");

	ControlFlow::Continue(())
}

pub fn handle_get_var(ctx: &mut ReplContext<'_>, name: String) -> ControlFlow<()> {
	let state = ctx.repl_state.lock().unwrap();
	match state.vars.get(&name) {
		Some(value) => println!("{value}"),
		None => eprintln!("Variable '{name}' not found"),
	}
	ControlFlow::Continue(())
}

// TODO: replace with simpler substring match
fn matches_pattern(text: &str, pattern: &str) -> bool {
	let mut text_chars = text.chars().peekable();
	let mut pattern_chars = pattern.chars().peekable();

	loop {
		match (pattern_chars.peek(), text_chars.peek()) {
			(None, None) => return true,
			(None, Some(_)) => return false,
			(Some(&'*'), _) => {
				pattern_chars.next();
				if pattern_chars.peek().is_none() {
					return true;
				}
				let rest_pattern: String = pattern_chars.clone().collect();
				while text_chars.peek().is_some() {
					let rest_text: String = text_chars.clone().collect();
					if matches_pattern(&rest_text, &rest_pattern) {
						return true;
					}
					text_chars.next();
				}
				return false;
			}
			(Some(&p), Some(&t)) => {
				if p == t {
					pattern_chars.next();
					text_chars.next();
				} else {
					return false;
				}
			}
			(Some(_), None) => return false,
		}
	}
}

#[cfg(test)]
mod tests {
	use std::sync::{Arc, Mutex};
	use crate::repl::state::ReplState;

	#[test]
	fn test_default_sets_variable_when_not_exists() {
		let state = Arc::new(Mutex::new(ReplState::new()));
		let mut repl_state_ref = state.lock().unwrap();
		
		// Simulate the default handler behaviour
		repl_state_ref.vars.entry("myvar".to_string()).or_insert("initial".to_string());
		
		assert_eq!(repl_state_ref.vars.get("myvar"), Some(&"initial".to_string()));
	}

	#[test]
	fn test_default_does_not_override_existing_variable() {
		let mut repl_state = ReplState::new();
		
		// Set initial value
		repl_state.vars.insert("myvar".to_string(), "original".to_string());
		assert_eq!(repl_state.vars.get("myvar"), Some(&"original".to_string()));

		// Try to default a different value
		repl_state.vars.entry("myvar".to_string()).or_insert("new".to_string());

		// Should still be original
		assert_eq!(repl_state.vars.get("myvar"), Some(&"original".to_string()));
	}

	#[test]
	fn test_set_overrides_default_value() {
		let mut repl_state = ReplState::new();

		// Set with default
		repl_state.vars.entry("myvar".to_string()).or_insert("default".to_string());
		assert_eq!(repl_state.vars.get("myvar"), Some(&"default".to_string()));

		// Override with set
		repl_state.vars.insert("myvar".to_string(), "override".to_string());
		assert_eq!(repl_state.vars.get("myvar"), Some(&"override".to_string()));
	}

	#[test]
	fn test_unset_then_default() {
		let mut repl_state = ReplState::new();

		// Set a value
		repl_state.vars.insert("myvar".to_string(), "initial".to_string());
		assert_eq!(repl_state.vars.get("myvar"), Some(&"initial".to_string()));

		// Unset it
		repl_state.vars.remove("myvar");
		assert_eq!(repl_state.vars.get("myvar"), None);

		// Default should now set it
		repl_state.vars.entry("myvar".to_string()).or_insert("after_unset".to_string());
		assert_eq!(repl_state.vars.get("myvar"), Some(&"after_unset".to_string()));
	}

	#[test]
	fn test_multiple_variables_independent() {
		let mut repl_state = ReplState::new();

		repl_state.vars.insert("var1".to_string(), "value1".to_string());
		repl_state.vars.entry("var2".to_string()).or_insert("value2".to_string());
		repl_state.vars.entry("var3".to_string()).or_insert("default3".to_string());

		assert_eq!(repl_state.vars.get("var1"), Some(&"value1".to_string()));
		assert_eq!(repl_state.vars.get("var2"), Some(&"value2".to_string()));
		assert_eq!(repl_state.vars.get("var3"), Some(&"default3".to_string()));

		// Trying to default var1 should not change it
		repl_state.vars.entry("var1".to_string()).or_insert("new".to_string());
		assert_eq!(repl_state.vars.get("var1"), Some(&"value1".to_string()));
	}

	#[test]
	fn test_default_with_whitespace_values() {
		let mut repl_state = ReplState::new();

		repl_state.vars.entry("var".to_string()).or_insert("value with spaces".to_string());
		assert_eq!(repl_state.vars.get("var"), Some(&"value with spaces".to_string()));
	}
}

use std::ops::ControlFlow;

use comfy_table::Table;

use super::state::ReplContext;

pub fn handle_set_var(ctx: &mut ReplContext<'_>, name: String, value: String) -> ControlFlow<()> {
	let mut state = ctx.repl_state.lock().unwrap();
	state.vars.insert(name, value);
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

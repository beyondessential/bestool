use rustyline::completion::Pair;

impl super::SqlCompleter {
	pub(super) fn complete_vars(&self, text_before_cursor: &str) -> Option<Vec<Pair>> {
		if !(text_before_cursor.trim_start().starts_with(r"\get ")
			|| text_before_cursor.trim_start().starts_with(r"\unset ")
			|| text_before_cursor.trim_start().starts_with(r"\set ")
			|| text_before_cursor.trim_start().starts_with(r"\default "))
		{
			return None;
		}

		let Some(repl_state_arc) = &self.repl_state else {
			return None;
		};

		let repl_state = repl_state_arc.lock().unwrap();

		// For \set and \default, only complete the variable name (first argument)
		let is_set_or_default = text_before_cursor.trim_start().starts_with(r"\set ")
			|| text_before_cursor.trim_start().starts_with(r"\default ");
		if is_set_or_default {
			// Check if we're on the first or second argument
			let after_cmd = if let Some(pos) = text_before_cursor.find(r"\set ") {
				&text_before_cursor[pos + 5..]
			} else if let Some(pos) = text_before_cursor.find(r"\default ") {
				&text_before_cursor[pos + 9..]
			} else {
				return Some(Vec::new());
			};

			// Count spaces to determine which argument we're on
			let space_count = after_cmd.chars().filter(|&c| c == ' ').count();
			if space_count > 0 && !after_cmd.ends_with(' ') {
				// We're on the second argument (value), don't complete
				return Some(Vec::new());
			}
		}

		let cmd_start = if let Some(pos) = text_before_cursor.find(r"\get ") {
			pos + 5
		} else if let Some(pos) = text_before_cursor.find(r"\unset ") {
			pos + 8
		} else if let Some(pos) = text_before_cursor.find(r"\set ") {
			pos + 5
		} else if let Some(pos) = text_before_cursor.find(r"\default ") {
			pos + 9
		} else {
			return Some(Vec::new());
		};

		let partial_var = text_before_cursor[cmd_start..].trim();

		let mut completions = Vec::new();
		for var_name in repl_state.vars.keys() {
			if var_name
				.to_lowercase()
				.starts_with(&partial_var.to_lowercase())
			{
				completions.push(Pair {
					display: var_name.clone(),
					replacement: var_name.clone(),
				});
			}
		}

		completions.sort_by(|a, b| a.display.cmp(&b.display));
		Some(completions)
	}
}

use rustyline::completion::Pair;

/// Valid modifier characters that can appear after \g
const MODIFIER_CHARS: &[char] = &['x', 'j', 'o', 'v', 'z'];

/// Generate query modifier completions based on what the user has typed
pub(super) fn generate_completions(current_word: &str) -> Vec<Pair> {
	// Handle just backslash - suggest all \g variants
	if current_word == "\\" {
		let mut completions = vec![Pair {
			display: "\\g".to_string(),
			replacement: "\\g".to_string(),
		}];

		// Add all single-modifier variants
		for &modifier in MODIFIER_CHARS {
			let mut completion = String::from("\\g");
			completion.push(modifier);
			completions.push(Pair {
				display: completion.clone(),
				replacement: completion,
			});
		}

		return completions;
	}

	if !current_word.starts_with("\\g") && !current_word.starts_with("\\G") {
		return Vec::new();
	}

	let after_g = &current_word[2..];
	let after_g_lower = after_g.to_lowercase();

	// Parse what modifiers are already present
	let mut used_modifiers = Vec::new();
	let mut has_set = false;
	let mut chars_iter = after_g_lower.chars().peekable();

	while let Some(ch) = chars_iter.peek() {
		if MODIFIER_CHARS.contains(ch) {
			used_modifiers.push(*ch);
			chars_iter.next();
		} else {
			break;
		}
	}

	// Check if "set" is being typed or already present
	let remaining: String = chars_iter.collect();
	if remaining == "set" || "set".starts_with(&remaining) {
		has_set = remaining == "set";
	}

	let mut completions = Vec::new();

	// If they've just typed \g, suggest all basic modifiers
	if after_g.is_empty() {
		completions.push(Pair {
			display: "\\g".to_string(),
			replacement: "\\g".to_string(),
		});
	}

	// Generate completions by adding each unused modifier
	for &modifier in MODIFIER_CHARS {
		if !used_modifiers.contains(&modifier) {
			let mut completion = String::from("\\g");
			for &m in &used_modifiers {
				completion.push(m);
			}
			completion.push(modifier);

			if completion
				.to_lowercase()
				.starts_with(&current_word.to_lowercase())
			{
				completions.push(Pair {
					display: completion.clone(),
					replacement: completion,
				});
			}
		}
	}

	// Add "set" variant if not already present and no remaining partial text
	if !has_set && remaining.is_empty() && !used_modifiers.is_empty() {
		let mut completion = String::from("\\g");
		for &m in &used_modifiers {
			completion.push(m);
		}
		completion.push_str("set");

		if completion
			.to_lowercase()
			.starts_with(&current_word.to_lowercase())
		{
			completions.push(Pair {
				display: completion.clone(),
				replacement: completion,
			});
		}
	}

	// Add partial "set" completions
	if !has_set && !remaining.is_empty() && "set".starts_with(&remaining) {
		let mut completion = String::from("\\g");
		for &m in &used_modifiers {
			completion.push(m);
		}
		completion.push_str("set");

		if completion
			.to_lowercase()
			.starts_with(&current_word.to_lowercase())
		{
			completions.push(Pair {
				display: completion.clone(),
				replacement: completion,
			});
		}
	}

	completions
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_generate_g_alone() {
		let completions = generate_completions("\\g");
		assert!(completions.iter().any(|c| c.display == "\\g"));
		assert!(completions.iter().any(|c| c.display == "\\gx"));
		assert!(completions.iter().any(|c| c.display == "\\gj"));
		assert!(completions.iter().any(|c| c.display == "\\go"));
		assert!(completions.iter().any(|c| c.display == "\\gv"));
		assert!(completions.iter().any(|c| c.display == "\\gz"));
	}

	#[test]
	fn test_generate_gx() {
		let completions = generate_completions("\\gx");
		assert!(completions.iter().any(|c| c.display == "\\gxj"));
		assert!(completions.iter().any(|c| c.display == "\\gxo"));
		assert!(completions.iter().any(|c| c.display == "\\gxv"));
		assert!(completions.iter().any(|c| c.display == "\\gxz"));
		assert!(completions.iter().any(|c| c.display == "\\gxset"));
		// Should not suggest x again
		assert!(!completions.iter().any(|c| c.display == "\\gxx"));
	}

	#[test]
	fn test_generate_gxj() {
		let completions = generate_completions("\\gxj");
		assert!(completions.iter().any(|c| c.display == "\\gxjo"));
		assert!(completions.iter().any(|c| c.display == "\\gxjv"));
		assert!(completions.iter().any(|c| c.display == "\\gxjz"));
		assert!(completions.iter().any(|c| c.display == "\\gxjset"));
		// Should not suggest x or j again
		assert!(!completions.iter().any(|c| c.display == "\\gxjx"));
		assert!(!completions.iter().any(|c| c.display == "\\gxjj"));
	}

	#[test]
	fn test_generate_gxz() {
		let completions = generate_completions("\\gxz");
		assert!(completions.iter().any(|c| c.display == "\\gxzj"));
		assert!(completions.iter().any(|c| c.display == "\\gxzo"));
		assert!(completions.iter().any(|c| c.display == "\\gxzv"));
		assert!(completions.iter().any(|c| c.display == "\\gxzset"));
	}

	#[test]
	fn test_generate_all_modifiers() {
		let completions = generate_completions("\\gxjovz");
		// All modifiers used, only set should be suggested
		assert!(completions.iter().any(|c| c.display == "\\gxjovzset"));
		// No individual modifiers should be suggested
		assert!(!completions.iter().any(|c| c.display == "\\gxjovzx"));
	}

	#[test]
	fn test_generate_partial_set() {
		let completions = generate_completions("\\gxs");
		assert!(completions.iter().any(|c| c.display == "\\gxset"));
	}

	#[test]
	fn test_generate_case_insensitive() {
		let completions = generate_completions("\\Gx");
		assert!(completions.iter().any(|c| c.display == "\\gxj"));
		assert!(completions.iter().any(|c| c.display == "\\gxo"));
	}

	#[test]
	fn test_non_g_command() {
		let completions = generate_completions("\\q");
		assert!(completions.is_empty());
	}

	#[test]
	fn test_backslash_only() {
		let completions = generate_completions("\\");
		// Should suggest \g and all single-modifier variants
		assert!(completions.iter().any(|c| c.display == "\\g"));
		assert!(completions.iter().any(|c| c.display == "\\gx"));
		assert!(completions.iter().any(|c| c.display == "\\gj"));
		assert!(completions.iter().any(|c| c.display == "\\go"));
		assert!(completions.iter().any(|c| c.display == "\\gv"));
		assert!(completions.iter().any(|c| c.display == "\\gz"));
	}
}

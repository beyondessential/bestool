use super::transaction::TransactionState;

pub fn build_prompt(
	database_name: &str,
	is_superuser: bool,
	buffer_is_empty: bool,
	transaction_state: TransactionState,
) -> String {
	let transaction_marker = match transaction_state {
		TransactionState::Error => "!",
		TransactionState::Active => "*",
		TransactionState::Idle | TransactionState::None => "",
	};

	let prompt_suffix = if is_superuser { "#" } else { ">" };

	if buffer_is_empty {
		format!("{database_name}={transaction_marker}{prompt_suffix} ")
	} else {
		format!("{database_name}->  ")
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_superuser_prompt() {
		let prompt = build_prompt("postgres", true, true, TransactionState::None);
		assert!(prompt.ends_with("# "));
	}

	#[test]
	fn test_transaction_markers() {
		let prompt_active = build_prompt("testdb", false, true, TransactionState::Active);
		assert!(prompt_active.contains("=*>"));

		let prompt_error = build_prompt("testdb", false, true, TransactionState::Error);
		assert!(prompt_error.contains("=!>"));
	}
}

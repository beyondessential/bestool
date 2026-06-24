use bestool_alertd::doctor::check::{Check, CheckStatus};

/// Severity order key for grouping completed checks: lower value = less severe,
/// renders nearer the top of the list and further from the result line.
pub fn severity_key(status: &CheckStatus) -> u8 {
	match status {
		CheckStatus::Pass => 0,
		CheckStatus::Skip(_) => 1,
		CheckStatus::Warning(_) => 2,
		CheckStatus::Broken(_) => 3,
		CheckStatus::Fail(_) => 4,
	}
}

/// Whether a completed check should appear in the result replay. By default the
/// replay shows only warning, broken, and failing checks; `--all` shows
/// everything (the live progress view always shows every check regardless).
pub fn keep_in_replay(status: &CheckStatus, show_all: bool) -> bool {
	if show_all {
		return true;
	}
	matches!(
		status,
		CheckStatus::Warning(_) | CheckStatus::Broken(_) | CheckStatus::Fail(_)
	)
}

/// Sort `results` into severity-grouped, alphabetical-within-group order.
pub fn sort_grouped(results: &mut [(Check, bool)]) {
	results.sort_by(|a, b| {
		severity_key(&a.0.status)
			.cmp(&severity_key(&b.0.status))
			.then_with(|| a.0.name.cmp(b.0.name))
	});
}

/// Filter `results` for the replay by the `show_all` flag, leaving them sorted.
pub fn filter_and_sort(results: &[(Check, bool)], show_all: bool) -> Vec<(Check, bool)> {
	let mut out: Vec<(Check, bool)> = results
		.iter()
		.filter(|(c, _)| keep_in_replay(&c.status, show_all))
		.cloned()
		.collect();
	sort_grouped(&mut out);
	out
}

#[cfg(test)]
mod tests {
	use super::*;

	fn pass(name: &'static str) -> (Check, bool) {
		(Check::pass(name, "ok"), true)
	}
	fn warn(name: &'static str) -> (Check, bool) {
		(Check::warning(name, "deg", "reason"), true)
	}
	fn broken(name: &'static str) -> (Check, bool) {
		(Check::broken(name, "broke", "reason"), true)
	}
	fn fail(name: &'static str) -> (Check, bool) {
		(Check::fail(name, "bad", "reason"), true)
	}
	fn skip(name: &'static str) -> (Check, bool) {
		(Check::skip(name, "not run", "reason"), true)
	}

	#[test]
	fn severity_key_orders_least_to_most() {
		assert!(severity_key(&CheckStatus::Pass) < severity_key(&CheckStatus::Skip("".into())));
		assert!(
			severity_key(&CheckStatus::Skip("".into()))
				< severity_key(&CheckStatus::Warning("".into()))
		);
		assert!(
			severity_key(&CheckStatus::Warning("".into()))
				< severity_key(&CheckStatus::Broken("".into()))
		);
		assert!(
			severity_key(&CheckStatus::Broken("".into()))
				< severity_key(&CheckStatus::Fail("".into()))
		);
	}

	#[test]
	fn sort_grouped_pass_skip_warn_broken_fail_alphabetical() {
		let mut results = vec![
			fail("zebra"),
			pass("delta"),
			warn("alpha"),
			broken("beta"),
			skip("gamma"),
			pass("charlie"),
			fail("apple"),
			warn("yak"),
		];
		sort_grouped(&mut results);
		let names: Vec<&str> = results.iter().map(|(c, _)| c.name).collect();
		assert_eq!(
			names,
			vec!["charlie", "delta", "gamma", "alpha", "yak", "beta", "apple", "zebra"]
		);
	}

	#[test]
	fn keep_in_replay_default_hides_pass_and_skip() {
		assert!(!keep_in_replay(&CheckStatus::Pass, false));
		assert!(!keep_in_replay(&CheckStatus::Skip("".into()), false));
		assert!(keep_in_replay(&CheckStatus::Warning("".into()), false));
		assert!(keep_in_replay(&CheckStatus::Broken("".into()), false));
		assert!(keep_in_replay(&CheckStatus::Fail("".into()), false));
	}

	#[test]
	fn keep_in_replay_show_all_keeps_everything() {
		assert!(keep_in_replay(&CheckStatus::Pass, true));
		assert!(keep_in_replay(&CheckStatus::Skip("".into()), true));
		assert!(keep_in_replay(&CheckStatus::Warning("".into()), true));
		assert!(keep_in_replay(&CheckStatus::Broken("".into()), true));
		assert!(keep_in_replay(&CheckStatus::Fail("".into()), true));
	}

	#[test]
	fn filter_and_sort_default_drops_pass_and_skip() {
		let input = vec![
			pass("a"),
			warn("b"),
			skip("c"),
			fail("d"),
			broken("e"),
			pass("f"),
		];
		let out = filter_and_sort(&input, false);
		let names: Vec<&str> = out.iter().map(|(c, _)| c.name).collect();
		assert_eq!(names, vec!["b", "e", "d"]);
	}
}

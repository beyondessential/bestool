//! Interactive retry for operations that depend on external state an operator
//! can fix by hand: a running service holding a file open, wrong permissions, a
//! busy device.
//!
//! On failure the error is shown and the operator is asked what to do. The
//! operation is re-run on each retry, so the underlying condition is genuinely
//! re-checked — declining to fix it just fails again, rather than letting the
//! caller skip the step. With no terminal to prompt on (a daemon, a redirected
//! stdin), the first failure is returned unchanged.

use std::io::{IsTerminal as _, Write as _};

use miette::{Context as _, IntoDiagnostic as _, Report, Result};
use tracing::warn;

/// Run `op`, prompting the operator to fix-and-retry or abort on failure. `what`
/// names the operation for the error and prompt (e.g. "stopping the postgres
/// cluster"). See the module docs for the non-interactive behaviour.
pub async fn retry<T, F>(what: &str, mut op: F) -> Result<T>
where
	F: AsyncFnMut() -> Result<T>,
{
	loop {
		let err = match op().await {
			Ok(value) => return Ok(value),
			Err(err) => err,
		};

		if !std::io::stdin().is_terminal() {
			return Err(err);
		}
		warn!("{what} failed: {err}");

		match prompt(what, &err, &[RETRY, ABORT], 'r')? {
			Some('r') => continue,
			_ => return Err(err),
		}
	}
}

/// Run `op`, and on failure offer the operator a third option beyond retry and
/// abort: run `recover` (a recovery action, e.g. resetting the WAL) and then
/// retry. `recover_label`/`recover_detail` name and explain that action in the
/// prompt. Each retry re-runs `op`; a failing `recover` aborts. Non-interactive
/// behaviour is as [`retry`]: the first failure is returned unchanged (the
/// recovery action is never taken without an explicit choice).
pub async fn retry_or_recover<T, Op, Rec>(
	what: &str,
	recover_label: &str,
	recover_detail: &str,
	mut op: Op,
	mut recover: Rec,
) -> Result<T>
where
	Op: AsyncFnMut() -> Result<T>,
	Rec: AsyncFnMut() -> Result<()>,
{
	loop {
		let err = match op().await {
			Ok(value) => return Ok(value),
			Err(err) => err,
		};

		if !std::io::stdin().is_terminal() {
			return Err(err);
		}
		warn!("{what} failed: {err}");

		let recover_choice = Choice {
			key: 'w',
			label: recover_label,
			detail: recover_detail,
		};
		match prompt(what, &err, &[RETRY, recover_choice, ABORT], 'r')? {
			Some('r') => continue,
			Some('w') => {
				recover().await?;
				continue;
			}
			_ => return Err(err),
		}
	}
}

/// One selectable option at a recovery prompt.
struct Choice<'a> {
	/// The key the operator presses to pick it.
	key: char,
	/// A short name.
	label: &'a str,
	/// A one-line explanation of what it does.
	detail: &'a str,
}

const RETRY: Choice<'static> = Choice {
	key: 'r',
	label: "retry",
	detail: "try again — e.g. after clearing the problem by hand",
};

const ABORT: Choice<'static> = Choice {
	key: 'a',
	label: "abort",
	detail: "give up and return the error",
};

/// Render the failure and `choices`, then read the operator's pick, returning
/// the chosen key (lowercased). An empty line picks `default`; end-of-input
/// (Ctrl-D) or an unreadable stdin gives `None` (the caller aborts). Re-prompts
/// on an unrecognised key.
fn prompt(what: &str, err: &Report, choices: &[Choice<'_>], default: char) -> Result<Option<char>> {
	eprintln!("\n{what} failed:\n{err:?}\n");
	for choice in choices {
		let marker = if choice.key == default {
			" (default)"
		} else {
			""
		};
		eprintln!(
			"  [{}] {}{} — {}",
			choice.key, choice.label, marker, choice.detail
		);
	}
	let keys = choices
		.iter()
		.map(|choice| {
			if choice.key == default {
				choice.key.to_ascii_uppercase()
			} else {
				choice.key
			}
			.to_string()
		})
		.collect::<Vec<_>>()
		.join("/");

	loop {
		eprint!("Choose [{keys}]: ");
		std::io::stderr().flush().ok();

		let mut answer = String::new();
		let read = std::io::stdin()
			.read_line(&mut answer)
			.into_diagnostic()
			.wrap_err("reading choice")?;
		if read == 0 {
			return Ok(None); // end of input: nothing more to answer with
		}
		let answer = answer.trim();
		if answer.is_empty() {
			return Ok(Some(default));
		}
		let key = answer.chars().next().unwrap().to_ascii_lowercase();
		if choices.iter().any(|choice| choice.key == key) {
			return Ok(Some(key));
		}
		eprintln!("'{answer}' is not one of the options.");
	}
}

#[cfg(test)]
mod tests {
	use std::sync::atomic::{AtomicU32, Ordering};

	use miette::miette;

	use super::*;

	/// A success on the first attempt never prompts, so it works without a TTY.
	#[tokio::test]
	async fn returns_immediately_on_success() {
		let calls = AtomicU32::new(0);
		let out: u32 = retry("noop", async || {
			calls.fetch_add(1, Ordering::SeqCst);
			Ok(7)
		})
		.await
		.unwrap();
		assert_eq!(out, 7);
		assert_eq!(calls.load(Ordering::SeqCst), 1);
	}

	/// The recovery action is never taken when the op succeeds.
	#[tokio::test]
	async fn recover_variant_returns_on_first_success() {
		let recovered = AtomicU32::new(0);
		let out: u32 = retry_or_recover(
			"noop",
			"recover",
			"do the thing",
			async || Ok(9),
			async || {
				recovered.fetch_add(1, Ordering::SeqCst);
				Ok(())
			},
		)
		.await
		.unwrap();
		assert_eq!(out, 9);
		assert_eq!(recovered.load(Ordering::SeqCst), 0);
	}

	/// Without a terminal a failure is returned after a single attempt (no loop,
	/// no recovery), matching the documented non-interactive behaviour.
	#[tokio::test]
	async fn fails_hard_without_a_terminal() {
		if std::io::stdin().is_terminal() {
			return; // can't exercise the non-interactive path with a real TTY attached
		}
		let calls = AtomicU32::new(0);
		let recovered = AtomicU32::new(0);
		let result: Result<()> = retry_or_recover(
			"always fails",
			"recover",
			"do the thing",
			async || {
				calls.fetch_add(1, Ordering::SeqCst);
				Err(miette!("boom"))
			},
			async || {
				recovered.fetch_add(1, Ordering::SeqCst);
				Ok(())
			},
		)
		.await;
		assert!(result.is_err());
		assert_eq!(calls.load(Ordering::SeqCst), 1);
		assert_eq!(recovered.load(Ordering::SeqCst), 0);
	}
}

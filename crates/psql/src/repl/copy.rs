use std::ops::ControlFlow;

pub fn handle_copy() -> ControlFlow<()> {
	eprintln!(r"The \copy command is not supported.");
	eprintln!(r"Instead, use \gz to execute a query without displaying output,");
	eprintln!(r"then use \re show to=path to export the result.");
	eprintln!();
	eprintln!(r"Example:");
	eprintln!(r"  SELECT * FROM users \gz");
	eprintln!(r"  \re show to=users.csv");
	ControlFlow::Continue(())
}

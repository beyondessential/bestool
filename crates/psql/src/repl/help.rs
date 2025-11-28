use std::ops::ControlFlow;

use comfy_table::{Row, Table};

pub fn handle_help() -> ControlFlow<()> {
	eprintln!("Metacommands:");
	let mut metacmds = Table::new();
	crate::table::configure(&mut metacmds);
	metacmds.load_preset(comfy_table::presets::NOTHING);

	metacmds.add_row(vec!["\\?", "Show this help"]);
	metacmds.add_row(vec!["\\help", "Show this help"]);
	metacmds.add_row(vec!["\\q", "Quit"]);
	metacmds.add_row(vec!["\\x", "Toggle expanded output mode"]);
	metacmds.add_row(vec!["\\W", "Toggle write mode"]);
	metacmds.add_row(vec![
		"\\R",
		"Toggle redaction mode (when redactions are configured)",
	]);
	metacmds.add_row(vec!["\\e [query]", "Edit query in external editor"]);
	metacmds.add_row(vec![
		"\\i <file> [var=val...]",
		"Execute commands from file",
	]);
	metacmds.add_row(vec![
		"\\o [file]",
		"Send query results to file (or close if no file)",
	]);
	metacmds.add_row(vec![
		"\\re list[+] [N]",
		"List the last N (default 20) saved results",
	]);
	metacmds.add_row(vec![
		"\\re show [params...]",
		"Display a saved result (n=N format=FMT to=PATH cols=COLS limit=N offset=N)",
	]);
	metacmds.add_row(vec![
		"\\debug [cmd]",
		"Debug commands (run \\debug for options)",
	]);
	metacmds.add_row(vec![
		"\\snip run <name> [var=val...]",
		"Run a saved snippet",
	]);
	metacmds.add_row(vec![
		"\\snip save <name>",
		"Save the preceding command as a snippet",
	]);
	metacmds.add_row(vec!["\\set <name> <value>", "Set a variable"]);
	metacmds.add_row(vec!["\\unset <name>", "Unset a variable"]);
	metacmds.add_row(vec!["\\get <name>", "Get and print a variable value"]);
	metacmds.add_row(vec![
		"\\vars [pattern]",
		"List variables (optionally matching pattern)",
	]);
	metacmds.add_row(vec![
		"\\list[+][!] <item> [pattern]",
		"List database objects (+ for details, ! for same connection)",
	]);
	metacmds.add_row(vec!["\\d{t,i,f,v,n,s}", "Aliases for \\list"]);
	metacmds.add_row(vec!["\\d[+][!] <name>", "Describe a database object"]);
	eprintln!("{metacmds}");

	eprintln!("Database objects (with \\list): table, index, function, view, schema, sequence");

	eprintln!("\nQuery modifiers (used after query):");
	let mut modifiers = Table::new();
	crate::table::configure(&mut modifiers);
	modifiers.load_preset(comfy_table::presets::NOTHING);

	modifiers.add_row(vec!["\\g", "Execute query"]);
	modifiers.add_row(vec!["\\gx", "...with expanded output"]);
	modifiers.add_row(vec!["\\gj", "...with JSON output"]);
	modifiers.add_row(vec!["\\gv", "...without variable interpolation"]);
	modifiers.add_row(vec!["\\gz", "...without displaying output"]);
	modifiers.add_row(vec!["\\go <file>", "...and write output to file"]);
	modifiers.add_row(vec![
		"\\gset [prefix]",
		"...and store cells in variables (one row only)",
	]);
	eprintln!("{modifiers}");

	eprintln!("\nModifiers can be combined, e.g. \\gxj for expanded JSON output");
	eprintln!(
		"Large result sets (>50 rows) are automatically truncated. Use \\re show to view more."
	);

	eprintln!("\nVariable interpolation:");
	let mut vars = Table::new();
	crate::table::configure(&mut vars);
	vars.load_preset(comfy_table::presets::NOTHING);

	vars.add_row(vec![
		"${name}",
		"Replace with variable value (errors if not set)",
	]);
	vars.add_row(vec![
		"${{name}}",
		"Escape: produces ${name} without replacement",
	]);
	eprintln!("{vars}");

	let mut fmts = Table::new();
	crate::table::configure(&mut fmts);
	fmts.load_preset(comfy_table::presets::NOTHING);

	fmts.set_header(Row::from(vec![
		r"\re show format=",
		r"\g modifier",
		"Description",
	]));
	crate::table::style_header(&mut fmts);

	fmts.add_row(vec!["table", r"\g", "Default table format"]);
	fmts.add_row(vec!["expanded", r"\gx", "One table per row"]);
	fmts.add_row(vec!["json", r"\gj", "Row=object, one per line"]);
	fmts.add_row(vec![
		"json-pretty",
		r"\gjx",
		"Array of objects, pretty-printed",
	]);
	fmts.add_row(vec!["csv", "", "CSV spreadsheet, with header"]);
	fmts.add_row(vec!["excel", "", "XLSX spreadsheet, only using to="]);
	fmts.add_row(vec!["sqlite", "", "SQLite database, only using to="]);
	eprintln!("{fmts}");

	ControlFlow::Continue(())
}

use std::ops::ControlFlow;

use comfy_table::Table;

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
	metacmds.add_row(vec![
		"\\describe[+][!] <name>",
		"Describe a database object (\\d for short)",
	]);
	eprintln!("{metacmds}");

	eprintln!("Database objects (with \\list): table, index, function, view, schema, sequence");

	eprintln!("\nQuery modifiers (used after query):");
	let mut modifiers = Table::new();
	crate::table::configure(&mut modifiers);
	modifiers.load_preset(comfy_table::presets::NOTHING);

	modifiers.add_row(vec!["\\g", "Execute query"]);
	modifiers.add_row(vec!["\\gx", "Execute query with expanded output"]);
	modifiers.add_row(vec!["\\gj", "Execute query with JSON output"]);
	modifiers.add_row(vec!["\\gv", "Execute query without variable interpolation"]);
	modifiers.add_row(vec![
		"\\go <file>",
		"Execute query and write output to file",
	]);
	modifiers.add_row(vec![
		"\\gset [prefix]",
		"Execute query and store results in variables",
	]);
	eprintln!("{modifiers}");

	eprintln!("\nModifiers can be combined, e.g. \\gxj for expanded JSON output");

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

	ControlFlow::Continue(())
}

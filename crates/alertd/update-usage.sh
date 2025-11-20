#!/bin/sh
set -eu

cd "$(dirname "$0")"

echo "Updating USAGE.md..."

# Use script to run in a PTY with fixed dimensions
# This ensures consistent line wrapping across different terminals
run_with_fixed_cols() {
	if command -v script >/dev/null 2>&1; then
		# Linux/BSD: Use script with a pseudo-terminal
		COLUMNS=80 script -q -c "$1" /dev/null | cat
	else
		# Fallback: just run the command
		eval "$1"
	fi
}

{
	echo "# Usage"
	echo ""
	echo "## Main Command"
	echo ""
	echo '```'
	run_with_fixed_cols "cargo run -p bestool-alertd --quiet -- --help"
	echo '```'
	echo ""
	echo "## Subcommands"
	echo ""

	for subcommand in run reload loaded-alerts pause-alert validate; do
		echo "### \`$subcommand\`"
		echo ""
		echo '```'
		run_with_fixed_cols "cargo run -p bestool-alertd --quiet -- $subcommand --help"
		echo '```'
		echo ""
	done
} > USAGE.md

echo "USAGE.md updated successfully"

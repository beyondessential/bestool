#!/bin/sh
set -eu

cd "$(dirname "$0")"

echo "Updating USAGE.md..."

# Use script to run in a PTY with fixed dimensions
# This ensures consistent line wrapping across different terminals
run_with_fixed_cols() {
	COLUMNS=80 NO_COLOR=1 eval "$1"
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

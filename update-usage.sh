#!/bin/sh
set -eu

cd "$(dirname "$0")"

genusage() {
	bin="$1"
	folder="$2"
	file="crates/$folder/USAGE.md"

	echo "  - $bin..."
	cargo run --bin "$bin" --quiet -- _docs > "$file"
	sed -i "s|$HOME|~|g" "$file"
}

echo "Updating USAGE.md files..."

genusage algae algae-cli
genusage bestool bestool
genusage bestool-alertd alertd
genusage bestool-psql psql

echo "All USAGE.md files updated successfully"

#!/bin/sh
set -eu

cd "$(dirname "$0")"

echo "Updating USAGE.md files..."

# bestool
echo "  - bestool..."
cargo run -p bestool --quiet -- _docs > crates/bestool/USAGE.md

# bestool-alertd
echo "  - bestool-alertd..."
cargo run -p bestool-alertd --quiet -- _docs > crates/alertd/USAGE.md

# algae-cli
echo "  - algae-cli..."
cargo run -p algae-cli --quiet -- _docs > crates/algae-cli/USAGE.md

# bestool-psql
echo "  - bestool-psql..."
cargo run -p bestool-psql --bin bestool-psql --quiet -- --_docs > crates/psql/USAGE.md

echo "All USAGE.md files updated successfully"

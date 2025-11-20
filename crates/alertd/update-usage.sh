#!/bin/sh
set -eu

cd "$(dirname "$0")"

echo "# Usage" > USAGE.md
echo "" >> USAGE.md
echo '```' >> USAGE.md
cargo run -p bestool-alertd --quiet -- --help >> USAGE.md
echo '```' >> USAGE.md

echo "USAGE.md updated successfully"

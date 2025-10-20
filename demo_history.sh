#!/bin/bash

# Demo script for bestool-psql history functionality

set -e

echo "=== bestool-psql History Demo ==="
echo

# Create a temporary directory for test history
TEST_DIR=$(mktemp -d)
HISTORY_DB="$TEST_DIR/demo_history.redb"

echo "History database: $HISTORY_DB"
echo

# Function to add a fake history entry for demo purposes
add_demo_entry() {
    local query="$1"
    local user="${2:-demo_user}"
    local writemode="${3:-false}"

    # We'll use the history module directly via a small Rust program
    # For now, just show what would be recorded
    echo "Would record: $query (user: $user, writemode: $writemode)"
}

echo "=== Step 1: Run psql with history enabled ==="
echo
echo "Command:"
echo "  cargo run --package bestool-psql --bin bestool-psql -- \\"
echo "    --history-path '$HISTORY_DB' \\"
echo "    -U demo_user \\"
echo "    -- -d tamanu_meta"
echo
echo "Then run some queries:"
echo "  SELECT 1;"
echo "  SELECT current_user;"
echo "  SELECT version();"
echo "  \\q"
echo

echo "=== Step 2: View history ==="
echo
echo "After running queries, view your history:"
echo

echo "Recent queries:"
echo "  cargo run --bin psql-history -- --history-path '$HISTORY_DB' recent"
echo

echo "List all history:"
echo "  cargo run --bin psql-history -- --history-path '$HISTORY_DB' list"
echo

echo "Show statistics:"
echo "  cargo run --bin psql-history -- --history-path '$HISTORY_DB' stats"
echo

echo "Export as JSON:"
echo "  cargo run --bin psql-history -- --history-path '$HISTORY_DB' export"
echo

echo "Clear history:"
echo "  cargo run --bin psql-history -- --history-path '$HISTORY_DB' clear"
echo

echo "=== Step 3: Test with a custom history file ==="
echo
echo "You can specify a custom history file location:"
echo "  cargo run --package bestool-psql --bin bestool-psql -- \\"
echo "    --history-path /path/to/custom/history.redb \\"
echo "    -- -d tamanu_meta"
echo

echo "=== Step 4: Disable history ==="
echo
echo "To run without history tracking:"
echo "  cargo run --package bestool-psql --bin bestool-psql -- \\"
echo "    --no-history \\"
echo "    -- -d tamanu_meta"
echo

echo "=== Features ==="
echo
echo "✓ History is stored in redb (embedded database)"
echo "✓ Each entry includes: query, user, writemode, timestamp"
echo "✓ History persists across sessions"
echo "✓ Rustyline integration for up-arrow navigation"
echo "✓ Separate tool (psql-history) to inspect/manage history"
echo "✓ Default location: ~/.local/state/bestool-psql/history.redb"
echo

echo "=== Cleanup ==="
echo "Test directory: $TEST_DIR"
echo "Run: rm -rf '$TEST_DIR'"
echo

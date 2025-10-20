#!/bin/bash
set -e

# Test script for history functionality

echo "=== Testing bestool-psql history functionality ==="
echo

# Create a temporary directory for test history
TEST_DIR=$(mktemp -d)
HISTORY_DB="$TEST_DIR/test_history.redb"

echo "Test history database: $HISTORY_DB"
echo

# Function to run a query and check history
run_test() {
    local query="$1"
    local description="$2"

    echo "Test: $description"
    echo "Query: $query"
    echo

    # Note: This would need to be run interactively or with expect
    # For now, just show the command that would be used
    echo "Command: echo '$query' | cargo run --package bestool-psql -- --history-path '$HISTORY_DB' -- -d tamanu_meta"
    echo
}

# Test cases
run_test "SELECT 1;" "Simple SELECT query"
run_test "SELECT current_user;" "Query to get current user"
run_test "SELECT version();" "Query to get PostgreSQL version"

echo "=== Manual test ==="
echo "Run the following command to test interactively:"
echo "  cargo run --package bestool-psql -- --history-path '$HISTORY_DB' -- -d tamanu_meta"
echo
echo "Then check the history was saved by examining the database."
echo

echo "To clean up the test directory:"
echo "  rm -rf '$TEST_DIR'"

# Keep the temp dir for manual testing
echo
echo "Test directory preserved at: $TEST_DIR"

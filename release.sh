#!/bin/sh
set -eu

cd "$(dirname "$0")"

# cargo-release wrapper for colocated jj+git repos.
#
# cargo-release drives git directly. In a colocated jj+git repo, jj's
# working-copy commit (@) sits on top of the git branch and holds any
# uncommitted changes. If @ is non-empty, or its parent is not the branch
# cargo-release expects, the release commit lands in the wrong place or trips
# cargo-release's dirty-state checks. After cargo-release moves the branch, jj
# still thinks @ is based on the old parent, which leaves stale state on the
# next jj command.
#
# This wrapper, when jj is present:
#   1. Refuses to run if @ has any changes.
#   2. Places @ as a fresh empty commit on main, so git HEAD == main.
#   3. Runs `cargo release "$@"`.
#   4. Imports the new git state into jj and re-parents @ onto the new main.
#
# When jj is absent (e.g. CI), it just execs cargo-release.

has_jj() {
	command -v jj >/dev/null 2>&1 && [ -d .jj ]
}

if ! has_jj; then
	exec cargo release "$@"
fi

if [ -n "$(jj diff -r @ --summary)" ]; then
	echo "error: jj @ has uncommitted changes; commit or abandon before releasing" >&2
	exit 1
fi

jj new main >/dev/null

set +e
cargo release "$@"
status=$?
set -e

jj git import >/dev/null
jj new main >/dev/null

exit "$status"

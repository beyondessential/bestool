#!/bin/sh
set -eu

cd "$(dirname "$0")/../.."
exec ./update-usage.sh

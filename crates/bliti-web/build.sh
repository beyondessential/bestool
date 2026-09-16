#!/bin/sh
# Build the wasm module and its bindings into www/pkg, which the page loads directly. No bundler:
# the output is an ES module the browser imports as it is.
set -eu
cd "$(dirname "$0")"
cargo build -p bliti-web --target wasm32-unknown-unknown --release
wasm-bindgen --target web --no-typescript --out-dir www/pkg \
	../../target/wasm32-unknown-unknown/release/bliti_web.wasm

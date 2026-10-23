#!/usr/bin/env bash
# Runs the audit rules' unit tests on the host (the .wasm target cannot run them).
set -euo pipefail
cd "$(dirname "$0")"
cargo test --target "$(rustc -vV | sed -n 's/^host: //p')" "$@"

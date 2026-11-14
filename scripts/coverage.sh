#!/usr/bin/env bash
# Tests under cargo-llvm-cov with every feature on, so every role's code is
# measured. Binary entry points (`main.rs`) are wiring only and excluded: no
# test can run them, and counting them at 0% made pstore's floor red on every
# push. Extra arguments go to `cargo llvm-cov`, e.g.
#   scripts/coverage.sh --fail-under-lines 95
#   scripts/coverage.sh --summary-only        # per-file view
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
if ! cargo llvm-cov --version >/dev/null 2>&1; then
  echo "coverage: cargo-llvm-cov is not installed: cargo install cargo-llvm-cov --locked" >&2
  exit 1
fi
exec cargo llvm-cov --workspace --all-features \
  --ignore-filename-regex '(^|[/\])src[/\]main\.rs$' "$@"

#!/usr/bin/env bash
# Mutation testing: would the tests notice if the code were wrong?
# Coverage says a line ran; a surviving mutant says no test constrains it.
#
#   scripts/mutants.sh            mutants in this branch's diff against origin/main (fast)
#   scripts/mutants.sh --all      every mutant in the workspace (slow; run on demand)
#   scripts/mutants.sh -- ARGS    extra arguments for cargo mutants
#
# Caveat (from pstore): the diff mode mutates the code under test only, so a
# change to tests alone yields no mutants. The full run covers that.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
if ! cargo mutants --version >/dev/null 2>&1; then
  echo "mutants: cargo-mutants is not installed: cargo install cargo-mutants --locked" >&2
  exit 1
fi
common=(--workspace --all-features --exclude 'crates/*/src/main.rs' --timeout-multiplier 3)
if [ "${1:-}" = "--all" ]; then
  shift
  exec cargo mutants "${common[@]}" "$@"
fi
base="${MUTANTS_BASE:-origin/main}"
diff_file="$(mktemp)"
trap 'rm -f "$diff_file"' EXIT
git diff "$base"...HEAD -- '*.rs' > "$diff_file"
if [ ! -s "$diff_file" ]; then
  echo "mutants: no Rust changes against $base"
  exit 0
fi
[ "${1:-}" = "--" ] && shift
cargo mutants "${common[@]}" --in-diff "$diff_file" "$@"

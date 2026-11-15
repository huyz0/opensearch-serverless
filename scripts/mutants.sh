#!/usr/bin/env bash
# Mutation testing: would the tests notice if the code were wrong?
# Coverage says a line ran; a surviving mutant says no test constrains it.
#
#   scripts/mutants.sh            every mutant in this branch's diff against origin/main
#                                 (run this locally before pushing a large change)
#   scripts/mutants.sh --pr       CI's budgeted run: at most MUTANTS_BUDGET (default 40)
#                                 of the diff's mutants, one shard chosen from the head
#                                 commit, within MUTANTS_TIME (default 7m)
#   scripts/mutants.sh --all      every mutant in the workspace (nightly; slow)
#   scripts/mutants.sh -- ARGS    extra arguments for cargo mutants
#
# In --pr mode a missed mutant fails; running out of time only warns, because
# the nightly sweep tests every mutant and the PR's Evidence names the local
# diff run. See docs/ci.md.
#
# Caveat (from pstore): the diff mode mutates the code under test only, so a
# change to tests alone yields no mutants. The full run covers that.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
. scripts/lib/py.sh
if ! cargo mutants --version >/dev/null 2>&1; then
  echo "mutants: cargo-mutants is not installed: cargo install cargo-mutants --locked" >&2
  exit 1
fi
common=(--workspace --all-features --exclude 'crates/*/src/main.rs' --timeout-multiplier 3)
if [ "${1:-}" = "--all" ]; then
  shift
  exec cargo mutants "${common[@]}" "$@"
fi
mode=diff
if [ "${1:-}" = "--pr" ]; then mode=pr; shift; fi
[ "${1:-}" = "--" ] && shift

base="${MUTANTS_BASE:-origin/main}"
diff_file="$(mktemp)"
trap 'rm -f "$diff_file"' EXIT
git diff "$base"...HEAD -- '*.rs' > "$diff_file"
if [ ! -s "$diff_file" ]; then
  echo "mutants: no Rust changes against $base"
  exit 0
fi
if [ "$mode" = diff ]; then
  exec cargo mutants "${common[@]}" --in-diff "$diff_file" "$@"
fi

count=$(cargo mutants "${common[@]}" --in-diff "$diff_file" --list | grep -c . || true)
shard=$(py scripts/mutants-plan.py "$count" "${MUTANTS_BUDGET:-40}" "$(git rev-parse --short=12 HEAD)")
if [ -z "$shard" ]; then
  echo "mutants: no mutants in the diff"
  exit 0
fi
echo "mutants: $count in the diff; testing shard $shard (budget ${MUTANTS_BUDGET:-40})"
rc=0
timeout "${MUTANTS_TIME:-7m}" cargo mutants "${common[@]}" --in-diff "$diff_file" --shard "$shard" "$@" || rc=$?
if [ "$rc" -eq 124 ]; then
  echo "::warning::mutants: shard $shard ran out of time; the nightly sweep covers every mutant"
  exit 0
fi
exit "$rc"

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
# Tests run under nextest when it is installed: every test is its own process
# spread across the cores, where `cargo test` runs a crate's test binaries one
# after another. Measured on oss-node (520 tests, 4 cores): 48 s against 27 s,
# the longest single test (18 s) being the floor. Doctests are not run by it;
# the PR's tests step runs them.
test_tool=()
if cargo nextest --version >/dev/null 2>&1; then test_tool=(--test-tool nextest); fi
# A test timeout auto-set from a cached baseline can be a few seconds, which a
# suite of socket tests with 5 s call deadlines outlasts when a mutant makes
# every call wait for its deadline: such a mutant is caught, not hung.
common=(--workspace --all-features --exclude 'crates/*/src/main.rs' --timeout-multiplier 3 --minimum-test-timeout 60 "${test_tool[@]}")
# MUTANTS_IN_PLACE=1 (CI's disposable runner) mutates the working tree itself,
# so the build the tests step left in target/ is reused instead of rebuilt in a
# copy, and skips the unmutated baseline, which that step already ran green.
# Measured on one shard of 11 mutants: 108 s in a copy, 48 s in place.
# Never set it on a checkout with uncommitted work.
# MUTANTS_BASELINE=run keeps the baseline (the nightly shards have no tests step
# before them).
if [ "${MUTANTS_IN_PLACE:-}" = "1" ]; then
  if [ "${MUTANTS_BASELINE:-skip}" = "run" ]; then
    common+=(--in-place)
  else
    common=(--workspace --all-features --exclude 'crates/*/src/main.rs' --in-place --baseline skip --timeout 60 "${test_tool[@]}")
  fi
fi
# cargo-mutants exits 3 when mutants only timed out (none missed, exit 2): a
# mutant that hangs the tests is detected by them, so it counts as caught.
run_mutants() {
  local rc=0
  cargo mutants "$@" || rc=$?
  if [ "$rc" -eq 3 ]; then
    echo "::warning::mutants: some mutants timed out; a mutant that hangs the tests is detected"
    return 0
  fi
  return "$rc"
}
if [ "${1:-}" = "--all" ]; then
  shift
  run_mutants "${common[@]}" "$@"
  exit $?
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
  run_mutants "${common[@]}" --in-diff "$diff_file" "$@"
  exit $?
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
[ "$rc" -eq 3 ] && { echo "::warning::mutants: some mutants timed out; a mutant that hangs the tests is detected"; rc=0; }
if [ "$rc" -eq 124 ]; then
  echo "::warning::mutants: shard $shard ran out of time; the nightly sweep covers every mutant"
  exit 0
fi
exit "$rc"

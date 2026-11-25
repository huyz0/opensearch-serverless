#!/usr/bin/env bash
# The nightly slow tier: every test marked `#[ignore = "slow"]`. The targets
# are read from the tree, so a new slow test runs nightly with no list to
# keep. A target holds only its own slow tests (an `ignore` of another
# reason, such as a measurement run by hand or one needing a Java node, is
# refused in the same file). Every failing target is shown, not the first.
#   scripts/slow-tests.sh [--list]
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
targets=()
while IFS= read -r file; do
  if grep -E '#\[ignore' "$file" | grep -qv 'ignore = "slow"'; then
    echo "slow-tests: $file mixes slow tests with other ignored ones" >&2
    exit 1
  fi
  crate_dir=${file%%/tests/*}
  rest=${file#*/tests/}
  target=${rest%%/*}
  target=${target%.rs}
  crate=$(sed -n 's/^name = "\(.*\)"/\1/p' "$crate_dir/Cargo.toml" | head -1)
  targets+=("$crate $target")
done < <(grep -rl --include=*.rs '#\[ignore = "slow"\]' crates/*/tests | sort)
printf '%s\n' "${targets[@]}" | sort -u > "${TMPDIR:-/tmp}/slow-targets.$$"
trap 'rm -f "${TMPDIR:-/tmp}/slow-targets.$$"' EXIT
if [ "${1:-}" = "--list" ]; then cat "${TMPDIR:-/tmp}/slow-targets.$$"; exit 0; fi
args=()
while read -r crate target; do args+=(-p "$crate" --test "$target"); done < "${TMPDIR:-/tmp}/slow-targets.$$"
[ "${#args[@]}" -gt 0 ] || { echo "slow-tests: none"; exit 0; }
exec cargo test --locked --no-fail-fast --all-features "${args[@]}" -- --ignored

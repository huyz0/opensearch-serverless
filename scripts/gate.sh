#!/usr/bin/env bash
# The gate, in one place. The pre-push hook and AGENTS.md defer to this
# script; the no-build checks are listed once, in scripts/tree-checks.sh,
# which the pre-commit hook and CI also run.
#
# `scripts/gate.sh --no-coverage` runs every step but the last (tests and
# coverage), for a machine that cannot build the coverage tooling: the steps CI
# fails on before it reaches the tests (fmt, clippy, each role alone, docs) are
# the ones a skipped gate lets through.
#
# Every step keeps its exit status (`set -o pipefail`; nothing is piped into
# `tail`, which would report tail's status instead of the step's).
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
. scripts/lib/py.sh

coverage=1
for arg in "$@"; do
  case "$arg" in
    --no-coverage) coverage=0 ;;
    *) echo "gate.sh: unknown argument $arg" >&2; exit 2 ;;
  esac
done

step() { echo "gate: $1"; }

step "cargo fmt --check"
cargo fmt --all --check

step "cargo clippy -D warnings (all features, includes the arithmetic lint)"
cargo clippy --workspace --all-targets --all-features -- -D warnings

step "cargo clippy, default features (the shipped reader-only build)"
cargo clippy --workspace --all-targets -- -D warnings

# A workspace build unifies features, so another crate switching a role on
# hides a module that only builds with its role's other features.
step "cargo check -p oss-node, each role alone (features are not unified)"
for features in "" "reader" "writer" "coordinator" "reader,writer" "reader,coordinator" "writer,coordinator"; do
  cargo check -p oss-node --no-default-features --features "$features"
done

step "tree checks (scripts/tree-checks.sh; each one: docs/mechanical-gates.md)"
scripts/tree-checks.sh

step "gen-crate-map --check (architecture crate map current, doc under 1,500 words)"
py scripts/gen-crate-map.py --check

# rustdoc link lints are reported by none of fmt, clippy or test.
step "cargo doc (rustdoc link lints)"
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --all-features --no-deps --document-private-items

if [ "$coverage" = 1 ]; then
  step "tests + >=95% line coverage"
  scripts/coverage.sh --fail-under-lines 95
else
  step "tests + coverage skipped (--no-coverage): CI runs them"
fi

echo "gate: ok"

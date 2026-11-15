#!/usr/bin/env bash
# The gate, in one place. The pre-push hook and AGENTS.md defer to this
# script; the no-build checks are listed once, in scripts/tree-checks.sh,
# which the pre-commit hook and CI also run.
#
# Every step keeps its exit status (`set -o pipefail`; nothing is piped into
# `tail`, which would report tail's status instead of the step's).
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
. scripts/lib/py.sh

step() { echo "gate: $1"; }

step "cargo fmt --check"
cargo fmt --all --check

step "cargo clippy -D warnings (all features, includes the arithmetic lint)"
cargo clippy --workspace --all-targets --all-features -- -D warnings

step "cargo clippy, default features (the shipped reader-only build)"
cargo clippy --workspace --all-targets -- -D warnings

step "tree checks (scripts/tree-checks.sh; each one: docs/mechanical-gates.md)"
scripts/tree-checks.sh

step "gen-crate-map --check (architecture crate map current, doc under 1,500 words)"
py scripts/gen-crate-map.py --check

# rustdoc link lints are reported by none of fmt, clippy or test.
step "cargo doc (rustdoc link lints)"
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --all-features --no-deps --document-private-items

step "tests + >=95% line coverage"
scripts/coverage.sh --fail-under-lines 95

echo "gate: ok"

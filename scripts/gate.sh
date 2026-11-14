#!/usr/bin/env bash
# The gate, in one place. AGENTS.md's command table, .githooks/pre-commit and
# CI all defer to this script, so the definition cannot drift between them.
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

step "selftest-checks (every check below fails on a planted violation)"
py scripts/selftest-checks.py

step "check-layering (downward-only crate dependencies, no cycles)"
py scripts/check-layering.py

step "check-crate-layout (README sections, AGENTS.md, src/lib.rs, workspace lints)"
py scripts/check-crate-layout.py

step "check-module-size (1,000-line limit; over 500 only on the shrinking baseline)"
py scripts/check-module-size.py

step "check-unsafe (unsafe_code stays forbidden)"
py scripts/check-unsafe.py

step "check-alloc (every request-path allocation states its bound; no unbounded channels)"
py scripts/check-alloc.py

step "check-s3-access (S3 clients only in oss-blob; every LIST justified)"
py scripts/check-s3-access.py

step "check-test-endpoints (tests run on emulators, never real AWS)"
py scripts/check-test-endpoints.py

step "check-arith-allows (every arithmetic allow carries an ARITH: proof)"
py scripts/check-arith-allows.py

step "check-dev-notes (no milestone, phase or task references in code)"
py scripts/check-dev-notes.py

step "check-port-map (every Rust file traces to its Java source or says why not)"
py scripts/check-port-map.py

step "check-invariants (every invariant names evidence that exists)"
py scripts/check-invariants.py

step "check-verified (every acceptance criterion of a finished spec has evidence)"
py scripts/check-verified.py

step "gen-crate-map --check (architecture crate map current, doc under 1,500 words)"
py scripts/gen-crate-map.py --check

step "ci-changes --self-test (CI path classification)"
py scripts/ci-changes.py --self-test

step "check-links (relative markdown links resolve)"
py scripts/check-links.py

# rustdoc link lints are reported by none of fmt, clippy or test.
step "cargo doc (rustdoc link lints)"
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --all-features --no-deps --document-private-items

step "tests + >=95% line coverage"
scripts/coverage.sh --fail-under-lines 95

echo "gate: ok"

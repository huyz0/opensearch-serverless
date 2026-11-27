#!/usr/bin/env bash
# The checks that read the tree and need no build, listed once. The pre-commit
# hook, scripts/gate.sh and CI all run this list, so a new check is added here
# and nowhere else. What each one enforces: docs/mechanical-gates.md.
#
#   scripts/tree-checks.sh           print every check's result
#   scripts/tree-checks.sh --quiet   print only a failing check's output
#
# Portable: Linux, macOS (bash 3.2) and Windows Git Bash.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
. scripts/lib/py.sh

checks="
selftest-checks.py
check-layering.py
check-crate-layout.py
check-module-size.py
check-unsafe.py
check-alloc.py
check-acked.py
check-s3-access.py
check-test-endpoints.py
check-budgets.py
check-extension-pins.py
check-analyzer-sites.py
check-arith-allows.py
check-test-binaries.py
check-pack-references.py
check-dev-notes.py
check-port-map.py
check-invariants.py
check-verified.py
check-harness.py
check-links.py
ci-changes.py --self-test
mutants-plan.py --self-test
ci-code-run.py --self-test
"

quiet=false
[ "${1:-}" = "--quiet" ] && quiet=true
printf '%s\n' "$checks" | while read -r script args; do
  [ -n "$script" ] || continue
  # shellcheck disable=SC2086
  if $quiet; then
    out=$(py "scripts/$script" $args 2>&1) || { printf '%s\n' "$out"; exit 1; }
  else
    py "scripts/$script" $args
  fi
done

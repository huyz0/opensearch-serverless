---
name: test-coverage
description: "WHAT: How tests are written here and the coverage and mutation floors. USE WHEN: writing or changing any module, adding an S3 interaction, or before declaring a change done."
---

# Tests and coverage

The testing rules, once (emulators only, the 95% floor, never weaken a threshold): [principles.md, Testing](../../../docs/principles.md#testing).

## Procedure

- **See it fail first:** write the test, break the code or run it before the fix, watch it go red, then make it pass.
- **Assert properties, not implementation:** the invariant a caller relies on, the request counts, the bytes, not "the loop ran four times".
- **Property tests** (`proptest`) for anything with an encode/decode or merge law.
- **Unit tests** in `#[cfg(test)] mod tests` for boundaries and errors: truncated input, bad header, zero, one, many, the limit and one past it.
- **No coverage theatre:** a test that calls code without asserting its result is deleted, not counted.
- **Read the per-file view** (`scripts/coverage.sh --summary-only`), not only the total. A slow test (over a second) gets `#[ignore = "slow"]` and runs nightly.

## Enforced by

- `scripts/coverage.sh --fail-under-lines 95` in `scripts/gate.sh` and nightly.
- `scripts/mutants.sh`: the PR's changed lines in CI (budgeted shard), the whole diff locally, everything nightly. A surviving mutant is a weak test.
- `scripts/check-test-endpoints.py` (no real AWS endpoints).
- Not mechanical: whether a test fails for the bug it names. Review.

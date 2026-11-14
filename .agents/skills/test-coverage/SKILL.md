---
name: test-coverage
description: "WHAT: How tests are written here and the ≥95% line coverage floor. USE WHEN: writing or changing any module, adding an S3 interaction, or before declaring a change done."
---

# Tests and coverage

## Rules

- **Emulators only.** S3 tests run against the in-process emulator (injectable latency and failures, request counting) or RustFS as a plain binary. Never real AWS, never Docker.
- **Assert properties, not implementation:** the invariant a caller relies on, the request counts, the bytes, not "the loop ran four times".
- **Property tests** (`proptest`) for anything with an encode/decode or merge law.
- **Unit tests** in `#[cfg(test)] mod tests` for boundaries and errors: truncated input, bad header, zero, one, many, the limit and one past it.
- **Every bounded resource** has a test at its limit and one past it.
- **≥95% line coverage**, workspace total, all features on; read the per-file view too (`scripts/coverage.sh --summary-only`).
- **No coverage theatre:** a test that calls code without asserting its result is deleted, not counted.
- **A test is seen failing** before it is trusted: break the code, watch it go red, restore.

## Enforced by

- `scripts/coverage.sh --fail-under-lines 95` in `scripts/gate.sh`.
- `scripts/check-test-endpoints.py` (no real AWS endpoints).
- Not mechanical yet: test strength. Mutation testing (`cargo mutants`) is planned once there is real logic ([docs/harness-review.md](../../../docs/harness-review.md)).

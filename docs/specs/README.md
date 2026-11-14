# Specs

One folder per feature: `docs/specs/<feature>/SPEC.md` before the code, `VERIFIED.md` beside it when the feature is done. A feature commit names its spec in a `Spec:` trailer.

- **SPEC.md** says what changes (not what the system is), the acceptance criteria as `- AC-<n>: ...` lines that a test, a gate or a number inside a bound can check, the S3 request budget per operation, and the risks. Under 200 lines. Start from [TEMPLATE.md](TEMPLATE.md).
- **VERIFIED.md** has one line per criterion, `- AC-<n>: ...`, naming the test function or command (in backticks) that proves it, or `NOT-RUN: <reason>`.

A criterion you cannot make checkable means the task is not understood yet; that is the signal to stop and ask, not to write a vague one.

`scripts/check-verified.py` fails when a VERIFIED.md misses a criterion or names a test that does not exist. It cannot tell whether the evidence is true.

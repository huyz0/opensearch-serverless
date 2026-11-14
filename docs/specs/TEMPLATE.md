# <Feature name>

## Serves

Which roadmap stage, invariant (`I-<n>`) or ADR this delivers. Three lines at most.

## Change

What changes, and explicitly what does not. Link docs instead of restating them.

## Acceptance criteria

- AC-1: <a property a test can assert, e.g. "a 10,000-document bulk issues exactly 1 PUT, asserted by the meter">
- AC-2: <...>

## Test plan

Per criterion: the test that must fail first, and the bug it would catch.

## S3 request budget

Requests per operation (PUT-class, GET-class, LIST, sequential depth). "Unchanged" is a valid answer; silence is not.

## Switch

The setting or feature this lands behind, its default, and the measurement that would flip it.

## Risks

What could make this wrong, and what would reveal it.

# oss-compat

The OpenSearch compatibility suite ([docs/compatibility.md](../../docs/compatibility.md)): the golden-fixture format, the normaliser for fields that legitimately vary, and a runner that replays fixtures against a target and compares responses.

## Owns
The OpenSearch compatibility suite: fixtures, normaliser, runner.

## Depends on
Nothing yet. It drives `oss-node` once REST exists, which is why it sits above every other crate.

## Entry points
`src/lib.rs`. The fixture format, normaliser and runner land in stage 1; fixtures live in `fixtures/compat/`.

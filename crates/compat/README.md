# oss-compat

The OpenSearch compatibility suite ([docs/compatibility.md](../../docs/compatibility.md)): the golden-fixture format, the normaliser for fields that legitimately vary, and a runner that replays fixtures against a target and compares responses.

## Owns
The OpenSearch compatibility suite: fixtures, normaliser, runner.

## Depends on
`serde_json` for the fixture format. It drives `oss-node` once REST exists, which is why it sits above every other crate.

## Entry points
- `load_dir` / `load_file` (`src/load.rs`): read fixtures with a bounded size.
- `Fixture` (`src/fixture.rs`): the format, documented for people in [fixtures/compat/README.md](../../fixtures/compat/README.md).
- `Target` (`src/target.rs`) and `run_suite` (`src/runner.rs`): replay fixtures and report per step.
- `normalise` (`src/normalise.rs`) and `compare` (`src/compare.rs`): what a step's pass or failure is made of.
- `tests/recorded.rs`: the checks on the recorded fixtures themselves.

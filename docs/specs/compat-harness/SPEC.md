# Compatibility harness

## Serves

Invariant I-13 (every in-scope endpoint matches OpenSearch's recorded responses) and the compatibility suite in [compatibility.md](../../compatibility.md). This builds the harness; the suite runs against `oss-node` once its first REST endpoint exists.

## Change

- `oss-compat` gains: the golden-fixture format, a bounded fixture loader, a normaliser, a comparator with readable differences, a `Target` trait and a runner.
- `fixtures/compat/` gains the recording script, its procedure, and the first recorded fixtures.
- Does not change: any other crate, the gate, CI. I-13 stays `NOT-YET` until `oss-node` serves REST; the fixture self-checks are named beside it.

## Acceptance criteria

- AC-1: a fixture file parses into steps of request (method, path, query, JSON or NDJSON body), expected response (status, selected headers, body) and normalisation rules; a rule without a non-empty `reason`, an unknown method, or a body that is both JSON and NDJSON is refused with an error naming the field.
- AC-2: the loader refuses a file larger than `MAX_FIXTURE_BYTES` with a size error, and accepts one of exactly that size; it never calls `fs::read` or `read_to_end` (clippy's disallowed methods).
- AC-3: a normalisation rule replaces the value at its pointer, including `*` segments over arrays and objects, in both the expected and the actual response; a rule that matches nothing in the recorded response is reported as a stale rule, and a field missing from the actual response is still reported as missing.
- AC-4: the comparator reports each difference with its JSON pointer and both values (changed, missing, unexpected), compares only the headers the fixture selects, and stops listing at `MAX_DIFFERENCES` with a count of the rest.
- AC-5: the runner against a fake target passes a step whose only difference is a normalised field, fails a step with a real body difference naming its pointer, and reports a target error without stopping the suite.
- AC-6: every fixture under `fixtures/compat/` loads, has a recorded response for each step, and every normalisation rule matches its recording; replaying the recordings through a fake target passes.
- AC-7: the recording script, run against stock OpenSearch at the version the Java shell builds on, writes the response of every step and the server's version into the fixture; it is not run by the gate.

## Test plan

- AC-1: parse tests per refusal; they fail if the parser accepts a missing reason or a doubled body.
- AC-2: size tests at the limit and one past it; they fail if the bound is off by one or missing.
- AC-3: normaliser tests over a wildcard pointer, a stale rule and a missing actual field; they fail if a rule silently hides a missing field.
- AC-4: comparator tests per difference kind, header selection and the cap.
- AC-5: runner self-tests with a fake target returning canned responses.
- AC-6: `recorded_fixtures_are_well_formed` and `recorded_fixtures_replay_cleanly` load the real files.
- AC-7: a manual run, recorded in VERIFIED.md with the OpenSearch build it ran against.

## S3 request budget

Unchanged: the harness makes no S3 requests.

## Switch

None: a test harness, not shipped behaviour.

## Risks

- A normalisation rule wide enough to hide a real difference. Each rule names its reason, a stale rule fails, and a missing field is never normalised away.
- Fixtures recorded from a different OpenSearch build than the shell's. Each fixture records the server's version and build hash.

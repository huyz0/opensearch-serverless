# The coordinator decides a rollover from the body and what the target says

## Serves

Parity plan R7.4, index admin REST: the decisions in the Java shell's `RolloverHandler` (indexadmin package, shell 429b4cbc) that need no node: the body's shape and conditions, the time values a condition reads, the name of the next index, the refusals on what the target is, and whether the conditions say to roll. The route, the alias and descriptor reads, the document count, the create, the alias swap and its rollback and the reply are separate pieces.

## Change

`oss-coordinator` gains `indices::time_value` (`TimeValue`, `parse`, `human`) and `indices::rollover` (`parse_body`, `Ask`, `Condition`, `next_name`, `backing_index_name`, `check_target`, `evaluate`).

## Acceptance criteria

- AC-1: `time_value::parse` reads `nanos`, `micros`, `ms`, `s`, `m`, `h`, `d` as `TimeValue.parseTimeValue` does (case-insensitive except that `M` is not minutes), plain `0`s and `-1`, and gives the Java's four refusal messages (missing or unrecognised unit, negative, fractional, unparsable); `human` prints a value as `toHumanReadableString(1)` does (`7d`, `1.5h`, `1.2h` for 75 minutes, `0s`, `-1`).
- AC-2: `next_name` increments a trailing number after the last dash, padded to six digits; a name with no dash, nothing after the last dash or a non-digit suffix gives none; a counter that cannot be incremented is refused 400 `illegal_argument_exception`, "numeric index suffix is exhausted at [N]". `backing_index_name` is `.ds-<stream>-<generation, six digits>`.
- AC-3: `parse_body` with no body asks for no conditions and no index shape; a member other than `conditions`, `settings`, `mappings`, `aliases`, or one that is not an object, is refused 400 `malformed_body`, "rollover field [k] must be a supported object-valued field"; a condition other than `max_age` and `max_docs` is refused 501 `unsupported_condition` ("store sizes are not reported here, so a size condition cannot be evaluated") when it begins `max_size` or `max_primary_shard_size`, else 400 `unsupported_condition`, "unknown condition [k]"; a `max_age` that is not a time value or a `max_docs` that is not a number is refused 400 `malformed_body` (this crate's words).
- AC-4: the settings, mappings and aliases of the body are read as a create request; a mapping of `{}` is no mapping; `shapes_index` is true when there is a mapping or any setting (a shard or replica count included), and `aliases_refusal` is the 501 `unsupported_setting` the shell gives when the body names aliases.
- AC-5: `check_target` refuses, in order: a target that is an index (400 `illegal_argument_exception`) or missing (404 `index_not_found_exception`); an alias with a mutation pending (409 `version_conflict_engine_exception`); a data stream given a new name or an index shape (400, core's words for each); an alias with no write index (400, "does not point to a write index").
- AC-6: `evaluate` gives each condition's key (`[max_age: <human>]`, `[max_docs: N]`) and whether it is met (age: now less created at, at least the age's milliseconds; docs: the count, asked for only when the condition exists, at least N); `max_age` on an index created at 0 is refused 501 `unsupported_condition`; a docs count that fails is that refusal; with no conditions it rolls, otherwise it rolls when any is met.

## Test plan

`crates/coordinator/tests/it/rollover.rs` and `crates/coordinator/tests/it/time_value.rs`: one test per criterion; the words asserted exactly.

## S3 request budget

Zero: nothing is read.

## Bounds

The tree is bounded by the body, which the edge bounds.

## Switch

None: the shell's rollover has no setting and no measured downside.

## Risks

The index-name suffix is ASCII digits where Java's `Character.isDigit` also takes other scripts' digits. A counter past 18 digits is refused as exhausted where Java throws a number format exception.

# The coordinator plans a mapping update

## Serves

Parity plan R7.4, index admin REST: the decisions in the Java shell's `MappingUpdateHandler` (indexadmin package, shell 429b4cbc) that need no node: the body requirement, the merge of the body over the stored mapping, whether anything changed, and the status each mapping refusal answers with. The route, the one-concrete-index check, the swap loop and the reply are separate pieces. The merge itself is `oss_doc::mapping::Mapping::merge`, which ports `MapperService.merge`.

## Change

`oss-coordinator` gains `indices::mapping_update`: `plan`, `MappingPlan`, `status_of` and `ATTEMPTS`.

## Acceptance criteria

- AC-1: no body or an empty one is refused 400 `missing_body`, "a mapping body is required".
- AC-2: the body is read as a put-mapping body (bare or wrapped in `_doc`) and merged over the stored mapping; the plan is the merged mapping as JSON without the `_doc` wrapper.
- AC-3: a body that changes nothing plans `Unchanged`; an index with no stored mapping always plans a replacement.
- AC-4: a refusal from the parse or the merge keeps its exception type and message and answers 400, except `mapper_exception` (500) and `unsupported_mapping` (501).
- AC-5: `ATTEMPTS` is 4.

## Test plan

`crates/coordinator/tests/mapping_update.rs`: one test per criterion; the merged JSON and the refusal words asserted exactly.

## S3 request budget

Zero: nothing is read.

## Bounds

The mapping text is at most `MAX_MAPPING_BYTES` and the merge is bounded by it.

## Switch

None: the shell's update has no setting and no measured downside.

## Risks

An unchanged result is judged on the normalised JSON, so a stored mapping written in another member order that merges to itself is left as it is, where the shell would rewrite it once.

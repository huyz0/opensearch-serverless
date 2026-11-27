# The coordinator decides what an update does to the document it read

## Serves

Parity plan R7.3, document APIs, fourth piece: the decisions in the Java shell's `DocumentUpdates` (ops package, shell 429b4cbc) that need no node and no script engine: which options an update accepts, how often it retries, what a read document and the request make of it, how a script's chosen operation is understood, and the condition the write carries. The merge of JSON bodies, the script, the reads and writes and the response are other pieces.

## Change

`oss-coordinator` gains `docs::update`: `validate` (the request's options), `attempts`, `plan` (from the document as read), `script_operation` (a script's `ctx.op`) and `write_condition`.

## Acceptance criteria

- AC-1: `validate` refuses, in this order: a negative `retry_on_conflict` (400 "retry_on_conflict must be non negative"); `scripted_upsert` with no script (400 "scripted_upsert was given without a script"); `require_alias` or active shards above one (501 "require_alias and active-shard counts above one are not supported on updates"); extra field values on the doc or upsert (501 "extra field values are not supported on updates").
- AC-2: `attempts` is one plus `retry_on_conflict`, saturating.
- AC-3: `plan` over a found document whose sequence number or primary term differs from a set `if_seq_no`/`if_primary_term` is a version conflict worded "[id]: version conflict, required seqNo [s], primary term [t]. current document has seqNo [a] and primary term [b]"; an unset condition (-2) never conflicts; a missing document never conflicts.
- AC-4: `plan` over a missing document is `Create` from the doc when `doc_as_upsert`, else from the upsert when there is one, else "document missing: [index]/[id]" (404), the doc winning over the upsert.
- AC-5: `plan` over a found document is `Script` when there is a script, else `Merge` with the request's `detect_noop`.
- AC-6: `script_operation` of a found document keeps `index` and `delete` and makes any other value a noop; of a missing document keeps `create` and makes any other value a noop; `none` and `noop` are noops.
- AC-7: `write_condition` gives the caller's own `if_seq_no`/`if_primary_term` when they set one; otherwise, for a found document, the sequence number and primary term just read, and for a missing one, "require absent" with no condition, so two concurrent upserts cannot both create.

## Test plan

`crates/coordinator/tests/it/update.rs`: one test per criterion; the words asserted exactly.

## S3 request budget

Zero: the decisions read nothing.

## Bounds

None to hold: one decision per update attempt.

## Switch

None: the shell's update has no setting and no measured downside.

## Risks

Scripts are not run here: a node without a script engine answers a script update as unsupported before it gets to `plan`.

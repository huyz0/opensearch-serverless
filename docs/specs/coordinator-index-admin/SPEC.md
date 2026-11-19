# The coordinator decides the index admin calls that read and delete

## Serves

Parity plan R7.4, index admin REST, last piece: the decisions in the Java shell's `IndexAdminHandler` (indexadmin package, shell 429b4cbc) for the index routes that need no node: which names are refused before any read, which view a GET renders, what a delete refuses and how a failed delete is told, the acknowledgement bodies, and the words of a failed alias after a create. The routes, the metadata reads, the create and delete themselves and the descriptor rendering are separate pieces.

## Change

`oss-coordinator` gains `indices::admin`: `check_name`, `view_of`, `View`, `delete_check`, `delete_failed`, `no_such_index`, `create_ack`, `delete_ack` and `alias_failed_after_create`.

## Acceptance criteria

- AC-1: `check_name` refuses a name that begins with an underscore with 501 `not_implemented` in the shell's words, for every method; with a create it refuses a name beginning `.ds-` with 400 `illegal_argument_exception` in the shell's words; underscore is decided first; anything else passes.
- AC-2: `view_of` gives the settings view for a path ending `/_settings` or when a setting name is given, the mapping view for a path ending `/_mapping` or `/_mappings`, and the full view otherwise; a setting name wins over a mapping path.
- AC-3: `delete_check` refuses an index that is the write index of a data stream with 400 `illegal_argument_exception`, "index [i] is the write index for data stream [s] and cannot be deleted"; nothing otherwise.
- AC-4: `delete_failed` is 409 `version_conflict_engine_exception`, "index [i] changed concurrently; nothing was deleted, retry", when the index existed and 404 `index_not_found`, "no such index: i" when it did not; `no_such_index` is the 404.
- AC-5: `create_ack` is `{"acknowledged":true,"shards_acknowledged":true,"index":"i","shards":N}` and `delete_ack` is `{"acknowledged":true,"index":"i"}`, the name written as a JSON string.
- AC-6: `alias_failed_after_create` keeps the alias refusal's status and type and says "index [i] was created, and its alias [a] was not: <reason>".

## Test plan

`crates/coordinator/tests/index_admin.rs`: one test per criterion; the words and bodies asserted exactly.

## S3 request budget

Zero: nothing is read.

## Bounds

Every output is bounded by the names in the request.

## Switch

None: no setting, no measured downside.

## Risks

None known beyond the pieces left to the node assembly.

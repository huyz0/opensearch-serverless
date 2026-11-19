# The coordinator decides the cluster settings calls

## Serves

Parity plan R7.5, cluster REST: the decisions in the Java shell's `ClusterSettingsHandler` (rest package, shell 429b4cbc) that need no node: what a settings update body may say, which values are refused before the register is written, and the body a read or an update answers with. The route, the register read and write (`oss-meta`) and the plugin settings are separate pieces; the plugin check is a closure the caller gives.

## Change

`oss-coordinator` gains `cluster::settings`: `parse_update`, `Shown`, `settings_body` and `ack_body`, and the keys of the settings this deployment checks.

## Acceptance criteria

- AC-1: `parse_update` refuses no body or an empty one with 400 `illegal_argument_exception`, "a settings update needs a 'persistent' object", the same for a body whose `persistent` is missing, null or not an object, and refuses a `transient` that is present and not an object with 400 `illegal_argument_exception`, "'transient' settings must be an object"; a body that is not a JSON object, or does not parse (a repeated member does not), is 400 `x_content_parse_exception`.
- AC-2: a body whose persistent and transient settings are both empty (an empty object, or objects holding only empty objects) is 400 `action_request_validation_exception`, "Validation Failed: 1: no settings to update;".
- AC-3: a transient setting is 501 `status_exception`, "transient settings are not supported; use persistent", decided before every check of a value.
- AC-4: the persistent settings come back flat and sorted by key: nested objects join their keys with `.`, a string is as written, a number and a boolean as Java prints them, a null is a removal (`None`).
- AC-5: a list value is 501 `status_exception`, "'<key>' is a list; list-valued settings are not supported on this register", the first key in key order; a null is never a list.
- AC-6: a value of `serverless.bucket` other than `auto` or `off` (case-insensitive, and empty), of `serverless.segment_placement` other than `segment_blob` or `node_pack` (likewise), of `serverless.tail_overlay` other than `true` or `false` (likewise) and of `serverless.refresh_visibility` other than `owner`, `serving` or `everywhere` (case-insensitive, and not empty) is 400 `status_exception` in the shell's words; a null is not checked.
- AC-7: `serverless.wal.node_log.commits.checkpoint_millis` must be a whole number of milliseconds, at least the larger of 1,000 and twice the commit period and at most 150,000, and the period itself must leave room for one; each refusal is 400 `status_exception` in the shell's words.
- AC-8: the plugin check runs last, over the flat settings, and its refusal is 400 `status_exception` with its words; it does not run when an earlier check refused.
- AC-9: `settings_body` is `{"persistent":{...},"transient":{}}` (and `"defaults":{}` when defaults were asked for) and `ack_body` is `{"acknowledged":true,"persistent":{...},"transient":{}}`; the settings are nested by their dotted names, or flat, one field a key, when asked; a list is an array of strings; a key that is also the start of another key is written flat beside it; nesting stops at a bounded depth.

## Test plan

`crates/coordinator/tests/cluster_settings.rs`: one test per criterion, words and bodies asserted exactly.

## S3 request budget

Zero: nothing is read.

## Bounds

Every output is bounded by the request body, which the edge bounds. Nesting recursion is capped at 64 levels.

## Switch

None: no setting, no measured downside.

## Risks

A key that is also a prefix of another is written flat, which is not pinned to the Java's order for that case. Recorded divergence: settings of the same first name are grouped, so a key like `a-b` can sort differently from the Java's tree.

# `function_score` script_score in Painless

## Serves

[ADR 0069](../../adr/0069-painless-source-compiles-in-rust-with-java-classes-as-functions.md) and [painless-front-end](../painless-front-end/SPEC.md): the first hook that sends Painless source to the engine's Rust front end. R14.6's remaining script gap.

## Change

- `script_score` accepts `lang: painless`, or no `lang` (stock's default), when the node's `serverless.scripting.painless.native_compile` switch is on (`OSS_SCRIPTING_PAINLESS_NATIVE_COMPILE`, default off). Off, the refusal is the one this node gave before, word for word.
- The source goes to the engine as written (`SOURCE_SCRIPT` blob). The engine compiles it, checks it runs exactly as Painless does, and refuses it with the reason when it does not: 400 `script_exception`, "compile error ..." for what Painless rejects, "does not serve" or "does not run ... as Painless does" otherwise.
- `doc['f']` fields read by a constant name are mapped from the index mapping: `long`/`integer`/`short`/`byte` as longs, `date`, `keyword` with doc values; a text field, an unsupported type or a field without doc values is a refusal, an unmapped one is the script's own `No field found` failure.
- Params are numbers (int, long, double), strings and booleans, as `params.name`.
- `script_fields`, `_script` sort, script queries, aggregations, update and ingest scripts are unchanged: they still refuse `lang: painless` (later hooks).
- The engine's bounds (source 65,535 bytes, loop counter, call depth, memory per invocation) apply as ADR 0069 states.

## Acceptance criteria

- AC-1: with the switch off (the default) `lang: painless` is refused as before; the variable `OSS_SCRIPTING_PAINLESS_NATIVE_COMPILE` turns it on and rejects a value that is not a boolean.
- AC-2: a Painless script scores hit for hit like the same arithmetic in an expression.
- AC-3: a script may declare variables and read `_score`.
- AC-4: Painless-rejected source, an unmapped or text field, and a stored script are 400 with a stated reason.
- AC-5: a Painless script costs the same object-store requests as the same search with an expression.

## Tests

AC-1: `crates/node/src/boot/config/tests/switches.rs`, `painless_is_served_only_while_the_switch_is_on` in `crates/reader/src/dsl/queries/function_score_tests.rs`, and `what_is_not_served_is_refused` in `crates/node/tests/it/function_score_script.rs` (switch off). AC-2 to AC-5: `crates/node/tests/script_score_painless.rs`, a process of its own because the switch is process-wide.

## S3 request budget

Unchanged: the source compiles in memory; doc values are read as for any scored search.

## Switch

`serverless.scripting.painless.native_compile`, default off, until the bounds of ADR 0069 (AC-4 to AC-7 of painless-front-end) hold on the node. The registry entry for the setting follows in its own change.

## Risks

A script the front end compiles that the runtime answers differently from Painless: `Spec::check` refuses what the corpus does not show equal, and the corpus floor is held in lucene-rust.

# `script_fields` in the expression language

## Serves

R14.6's remaining "runs no script", after [function-score-script](../function-score-script/SPEC.md) and [script-sort](../script-sort/SPEC.md): `"script_fields":{"name":{"script":{"lang":"expression",..}}}` (`ScriptFieldsContext`). Only the expression language runs on this node, for the reason the first spec gives.

## Change

- `oss_reader::fetch::script_fields` parses the object and resolves each entry, over the mapping, into an engine expression script; `SearchSource` gains `script_fields`.
- Each hit's `fields` object carries `"<name>":[<number>]`, the script's double written as Java writes it, beside any `docvalue_fields` (docvalue entries first, then script fields in request order). A document with no value for a field the script reads still gets a number: the expression reads a missing value as `0`.
- At most 32 script fields (`index.max_script_fields`' default); more is 400 `illegal_argument_exception` "Trying to retrieve too many script_fields. Must be less than or equal to: [32] but was [n]. This limit can be set by changing the [index.max_script_fields] index level setting."
- **Refused** (400, "is not supported by this node yet"): a script that reads `_score`, `ignore_failure`, `top_hits` and inner hits `script_fields`, and everything `script_score` refuses.
- No setting: the change adds an accepted shape with no measured downside.

## Acceptance criteria

- AC-1: a script field is written under `fields` as a one-element array holding the expression's value for that hit.
- AC-2: it sits beside `docvalue_fields` in one `fields` object.
- AC-3: more than 32 script fields is refused with the stock message.
- AC-4: every other refusal answers 400 with the stated reason.
- AC-5: a search with script fields adds no object-store request over the same search with `docvalue_fields` of the field.

## Tests

`crates/node/tests/it/script_fields.rs`; unit tests beside `fetch/script_fields.rs`.

## S3 request budget

None added: the script reads doc values as `docvalue_fields` does.

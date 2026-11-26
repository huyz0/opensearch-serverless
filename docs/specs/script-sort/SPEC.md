# `_script` sort in the expression language

## Serves

R14.6's remaining "runs no script", after [function-score-script](../function-score-script/SPEC.md): `"sort":[{"_script":{"type":"number","script":{"lang":"expression",..},"order":"asc"}}]` (`ScriptSortBuilder`). Only the expression language runs on this node, for the reason that spec gives.

## Change

- `oss_reader::dsl::SortKey::Script` carries the parsed script and the order; the script object, its limits and its refusals are those of `script_score`.
- The key is the engine's `PainlessSort` over an expression: the script's number for every document, never missing, ordered as `Double.compare` orders. A hit's `sort` value is that number written as Java writes a double (`1.0E7`, `2.0`).
- A new `SortValue::Double` carries it, so the coordinator merges script keys of several shards as it merges any other key.
- `type: number` only. **Refused** (400, "is not supported by this node yet"): `type: string`, `nested`, `mode`, a `_script` key with `search_after`, in an inner hits `sort` and in `top_hits`, and everything `script_score` refuses.
- No setting: the change adds an accepted shape with no measured downside.

## Acceptance criteria

- AC-1: sorting by an expression over a numeric doc value orders the hits as the same sort over that field does, ascending and descending.
- AC-2: a hit's `sort` array holds the script's number as a Java double, beside a field key in the same sort.
- AC-3: a two-shard index sorts through the coordinator the same as one shard.
- AC-4: every refusal above answers 400 with the stated reason and runs nothing.
- AC-5: a search with a script sort costs the same object-store requests as the same search sorted by the field.

## Tests

`crates/node/tests/it/script_sort.rs`; unit tests beside `dsl/sort.rs`.

## S3 request budget

None added: the script reads doc values as any sort does.

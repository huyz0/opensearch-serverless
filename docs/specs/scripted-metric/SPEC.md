# The `scripted_metric` aggregation

## Serves

Parity plan R10.2, aggregations: `scripted_metric` as `ScriptedMetricAggregationBuilder` parses it: `init_script`, `map_script`, `combine_script` and `reduce_script` (each `source` text or an object with `source` and `params`, `lang` of `painless` or absent) and `params`. Served only when `serverless.scripting.painless.native_compile` is on; with it off the aggregation is refused as before.

## How it runs

- Per shard: `init_script` runs once with `params` and an empty `state` map; `map_script` runs for each matched document with `params`, `state`, `doc` and `_score`; `combine_script` runs once with `params` and `state` and returns the shard's value (any map, list, string, number, boolean or null; the usual `state` is a map or a number).
- At the coordinator: `reduce_script` runs once with `params` and `states`, the list of every shard's combined value in shard order, and its return value is the aggregation's `value`. Without a `reduce_script` (as OpenSearch allows) the `value` is the list of the combined values.
- A script that does not compile, that the runtime does not run exactly as Painless does, or that throws fails the shard (or the reduce) with a 400 `script_exception`.

## Bounds

- The state a shard holds is one value; its size is bounded by the node's script memory bound per invocation (ADR 0069) and, as a shard result, by the aggregation result size bound a shard answer already has. A script that builds more than the bound fails with a 400, it is not truncated.
- `map_script` runs once per matched document with no more than the loop and memory bounds of ADR 0069 each; the documents are visited in order, one segment at a time, as other aggregations visit them.
- Not served, refused with a stated reason: the legacy `params._agg`, `_aggs`, stored scripts (`id`), `lang` other than painless, a `scripted_metric` inside a `terms` or other bucket (a top-level aggregation only), and `_score` use (the map script is run without scores).

## S3 request budget

None of its own: it reads the doc values the search already reads.

## Switch

`serverless.scripting.painless.native_compile`, off by default.

## Acceptance criteria

- AC-1: the front end compiles `init_script`, `map_script`, `combine_script` and `reduce_script` as their contexts (`params`, `state`, `doc`, `states`), and the runners return the state, the shard value and the reduced value.
- AC-2: the map script runs once per matched document over one `state` shared by every segment of the shard, and the state it leaves is handed back whole; a state that is not a plain value, or over the state bound, fails the aggregation.
- AC-3: the reader parses the four scripts and `params`, and refuses `params._agg`, stored scripts, a `lang` other than painless and a `scripted_metric` under a bucket aggregation, each with a stated reason; with the switch off the aggregation is refused as before.
- AC-4: a shard answers its combined value, and the coordinator runs `reduce_script` over every shard's value in shard order and renders it as the aggregation's `value`; without a `reduce_script` the `value` is the list of combined values.
- AC-5: a script that does not compile, is refused by the runtime's check, or throws fails with a 400 `script_exception`.

## Risks

Scripts that the front end compiles and the runtime runs differently from Painless are refused by `Spec::check`, as for other contexts. The state crossing a node boundary has no Java wire form, so a scripted_metric is answered only by the node that parses the request.

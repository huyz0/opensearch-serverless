# PUT /{index}/_settings over the descriptor

## Serves

Parity plan R14.4, index admin REST: the node route for a settings update. The decisions (body shapes, unknown keys, refusals, merge) are `oss-coordinator`'s `indices::settings_update` ([coordinator-settings-update](../coordinator-settings-update/SPEC.md)); this route reads the registry behind its `Scope`, runs the descriptor compare-and-swap loop and answers.

## Change

`oss-node` gains `settings_route`: `PUT /{index}/_settings` and `PUT /_settings`. A `Scope` over the node's registry classifies each key: our own keys by their validators, the core keys `core_settings` accepts as dynamic, the keys core declares static as static, every other key unknown. The route flattens and checks the body, then swaps the index record with the merged settings and `settings_version + 1`. Behaviour-changing settings stay refused 501 until each is implemented for real.

## Acceptance criteria

- AC-1: an update of an index that exists stores the merged settings in its descriptor, bumps `settings_version` by one and answers 200 `{"acknowledged":true}`; a read of the descriptor shows the new value.
- AC-2: a body the coordinator refuses (missing, malformed, empty, unknown key, refused ask, static key) is answered with its status, type and words, and the descriptor is unchanged.
- AC-3: an update that changes nothing is acknowledged and writes nothing: `settings_version` stays.
- AC-4: a missing index is 404 `index_not_found_exception`; `PUT /_settings` applies to every index and costs requests in proportion to the indices it changes.
- AC-5: a swap that loses to another writer is retried against the generation just read, at most `ATTEMPTS` times, then answered 409.
- AC-6: a null removes the key; `preserve_existing=true` keeps keys the descriptor already holds.
- AC-7: the route has an authorization row; an index setting update needs the index write-admin action.

## Test plan

Unit tests beside the module for the `Scope`; `crates/node/tests/server/settings_route.rs` for AC-1 to AC-6 against the in-process server, written first and seen red; `every_registered_route_has_an_action_row` for AC-7.

## S3 request budget

One update: 1 GET of the record and 1 PUT (the swap); a lost swap adds one GET and one PUT. An unchanged update: 1 GET. No LIST: `PUT /_settings` resolves indices from the descriptor listing the node already holds in memory.

## Bounds

The body is bounded by the edge and by `MAX_BODY`; one record buffer a request, `buffer_bytes`, reserved before use.

## Switch

None: the shell's update has no setting and no downside.

## Risks

`PUT /_settings` over many indices must not hold one buffer per index: one buffer is reused index by index.

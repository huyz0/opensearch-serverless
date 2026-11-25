# Core index settings a create accepts and stores

## Serves

R7.4, the core index settings registry behind a create. [index-create-settings](../index-create-settings/SPEC.md) accepted three settings the node reads and refused every other key 501, so a client that sets `index.codec` or a slow log threshold in its create body, or has one in a template, could not create the index. Many core settings tune things a serverless node does not have (a local translog, replica allocation, slow logs, cache choices); storing them changes nothing the client relied on.

## Change

`oss-node`'s `core_settings` registry names the core settings a create accepts, checks each value as core does, and stores it in the descriptor beside the three the node reads (keys sorted). Accepted: `index.codec` (`default`, `best_compression`, `lz4`, `zlib`, `zstd`, `zstd_no_dict`), `index.translog.durability` (`request`, `async`), `index.translog.sync_interval`, `index.unassigned.node_left.delayed_timeout`, `index.search.idle.after`, `index.gc_deletes` (time values), `index.priority` (a whole number), `index.queries.cache.enabled`, `index.requests.cache.enable` (booleans), `index.shard.check_on_startup` (`false`, `true`, `checksum`), `index.search.slowlog.level` and `index.indexing.slowlog.level` (`warn`, `info`, `debug`, `trace`), `index.routing.allocation.{include,exclude,require}.<attribute>` (non-empty text) and `index.search.slowlog.threshold.*` and `index.indexing.slowlog.threshold.*` (a time value or `-1`).

A bad value is refused 400 `illegal_argument_exception` naming the key. Every other key is still refused 501 `unsupported_index_option`, naming each, because storing a setting that changes behaviour and ignoring it would break what the client set: `index.blocks.*`, `index.max_result_window` and the other limits, `index.analysis.*`, `index.mapping.*`, `index.similarity.*`, and the rest. Those arrive with the behaviour they promise.

## Acceptance criteria

- AC-1: a create (or a template's create) with accepted core settings, flat or nested, succeeds and the descriptor's settings object holds them with the others, keys sorted. Test: `core_settings_the_node_only_stores_are_accepted_and_kept`.
- AC-2: the accepted keys and prefixes are exactly those above; a prefix alone is not a key. Test: `a_key_is_accepted_by_name_or_under_a_prefix_but_not_the_prefix_alone`.
- AC-3: a value that fails its check is refused 400 `illegal_argument_exception` and no descriptor is made. Tests: `a_bad_core_value_is_refused_and_writes_nothing`, `each_check_takes_its_values_and_refuses_the_rest`.
- AC-4: a core setting that would change behaviour (`index.blocks.write`, `index.max_result_window`) is still refused 501 naming it, and nothing is created, even beside an accepted one. Test: `a_core_setting_that_would_change_behaviour_is_still_refused`.

## S3 request budget

Unchanged: the settings ride in the descriptor create a create already makes. 0 LIST.

## Switch

None: it turns a refusal into a success for settings that have no effect on this node; no request, read or write path changes.

## Risks

- A client that sets one of these expecting an effect (`index.translog.durability: async` for speed) gets none; a write is durable once its log blob lands whatever the value. [compatibility](../../compatibility.md) says so.

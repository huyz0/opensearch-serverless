# Settings given at create, by the request or by a template

## Serves

The snapshots, templates, ingest and auth milestone. A create refuses every index setting it is given ("index settings at create are not supported by this node yet"), and a template that carries a setting makes every create it matches fail. A client that sets `index.refresh_interval` in its create body, or has it in a template, cannot create the index at all. The node already reads three settings from the descriptor (`index.refresh_interval` for the publish age, `index.routing_partition_size` and `index.number_of_routing_shards` for routing), so these are stored and used, not only accepted.

## Change

The settings of the create body, after a matching template's are laid under them (the request wins, as templates already do), are checked and stored in the descriptor's settings object beside the tenancy settings (`index.serverless.group`, the cost tier and budget):

- `index.refresh_interval`: a time value (`1s`, `500ms`, `5m`) or `-1`, read as the publish age reads it.
- `index.routing_partition_size`: an integer of at least 1, and less than `number_of_shards` unless the index has one shard, where only 1 is allowed.
- `index.number_of_routing_shards`: an integer that is a multiple of `number_of_shards` and at least as large.

Keys are read flat or nested (`{"index": {"refresh_interval": "5s"}}`), with or without the `index.` prefix, as a create body is read today. A value of null is dropped. The descriptor's settings object is flat, keys sorted.

Any other key is refused 501 `unsupported_index_option`, naming each key sorted ("index setting [a, b] is not supported by this node at create yet"), whether the request or a template carried it. The refusal is the node's own limit, not a claim the key is wrong: no settings registry for core keys exists here yet.

## Acceptance criteria

- AC-1: a create that gives `index.refresh_interval`, `index.routing_partition_size` and `index.number_of_routing_shards` (valid, flat or nested) succeeds, and the descriptor's settings object holds them with the tenancy settings the request also gave. Test: `a_create_stores_the_settings_the_node_reads`.
- AC-2: an invalid `index.refresh_interval` (not a time value and not `-1`; `0` is a time value) is refused 400 `illegal_argument_exception` naming the key, and no descriptor is made. Test: `a_bad_refresh_interval_is_refused_at_create`.
- AC-3: `index.routing_partition_size` below 1, not below the shard count (more than 1 shard), or not 1 (one shard), and an `index.number_of_routing_shards` that is below the shard count or not a multiple of it, are refused 400 `illegal_argument_exception` naming the key. Test: `routing_settings_are_checked_against_the_shard_count`.
- AC-4: any other key is refused 501 `unsupported_index_option` naming every such key, sorted, and no descriptor is made. Test: `other_settings_are_refused_and_named`.
- AC-5: the settings of the winning template reach the create; a key the request also gives takes the request's value; a template key outside the three is refused as in AC-4. Tests: `a_templates_settings_apply_and_the_request_wins`, `a_template_setting_the_node_does_not_read_is_refused_not_dropped`.
- AC-6: an index created with `index.refresh_interval` gets that publish age when its shard opens, and one created with a routing partition size routes by it. Test: `a_create_stores_the_settings_the_node_reads` (the publish age read back from the stored settings; the routing keys are stored as numbers, which the router reads).
- AC-7: a create that gives no settings, or only tenancy settings, writes the same descriptor as before. Existing create tests pass unchanged.

## S3 request budget

Unchanged: a create's requests are the same; the settings ride in the descriptor create it already makes. No LIST.

## Bounds

The create body is bounded at 1 MiB by the edge; the settings are a subset of it.

## Switch

None: it turns a refusal into a success for three settings the node already reads, adds no request and changes no read or write path of an index that has none of them.

## Risks

A setting the shell stores but this node ignores is not accepted here (it is refused, naming the key), so the descriptor never holds a value nothing reads. A template written for a classic cluster that sets `number_of_replicas` or `codec` still fails a create on this node until the registry exists; that is the existing behaviour, now named.

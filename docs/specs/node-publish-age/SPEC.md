# An index's refresh interval bounds how long its writes wait to be published

## Serves

R8.5 lazy publish: the publish policy already takes a per-shard age ([publish loop](../node-publish-loop/SPEC.md)); the node passed the node-wide age for every shard. An index that sets `index.refresh_interval` now bounds its own shards' wait, as the shell's `BackgroundReconciler.setLazyPublish` does.

## Change

- `oss_node::writer::publish_age_of` reads `index.refresh_interval` from descriptor settings JSON (flat or nested): a time value is `PublishAge::Millis`, `-1` is `PublishAge::Never` (published by operation count or on release only), and no setting, or one that does not parse, is `PublishAge::Node`.
- `HeadOpener` reads it from the descriptor it already reads to open the shard and sets it on the role (`WriterRole::set_publish_age`), for a fresh shard and for a taken one. `Publishing` keeps one entry per shard that has a non-default age and drops it when the shard is released.

## Acceptance criteria

- AC-1: `publish_age_of` answers `Millis` for a time value (`ms`, `s`, `m`, `h`, `d`, and `0`), `Never` for `-1`, and `Node` for an absent, malformed, non-string or unparsable setting.
- AC-2: a shard of an index with `refresh_interval` of `1h` is not published by age at the node's one second, one with `-1` is not, and one that sets none is.
- AC-3: the setting is read when the shard is opened, from the descriptor the opener already reads, so it adds no blob request.

## Test plan

`crates/node/tests/roles/publish_age.rs` (AC-1) and `tests/roles/writer_opener.rs` (AC-2, over a role that publishes and an opener over real descriptors and heads; observed failing with the call that sets the age removed). AC-3 is by construction: no store call is added.

## S3 request budget

None added: the settings come from the descriptor the open already reads.

## Risks

- A settings update after a shard is open does not change its age until the shard is opened again. The node has no `refresh_interval` update route yet (it answers 501).

# Verified: node-publish-age

- AC-1: `a_refresh_interval_is_read_flat_nested_or_not_at_all` in crates/node/tests/roles/publish_age.rs
- AC-2: `an_indexs_refresh_interval_sets_how_long_its_shards_wait_to_publish` in crates/node/tests/roles/writer_opener.rs
- AC-3: NOT-RUN: by construction; `HeadOpener::open` adds no store call.

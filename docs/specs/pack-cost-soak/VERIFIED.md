# Pack cost soak: verification

Emulator, in-process, debug build, one machine: counts are exact for the run, heap and timing provisional. Seen failing first with packs off (`max_blob_bytes` 1): 6,000 PUT against 3,001 expected.

- AC-1: `a_thousand_shards_publish_into_shared_packs` (writer, `tests/pack_soak.rs`): 3,000 publishes, 3,061 PUT, 0 LIST, 1.02 requests a publish against the blob mode's 2.0.
- AC-2: `a_thousand_shards_publish_into_shared_packs` (writer, `tests/pack_soak.rs`): PUT = 3,000 swaps + 60 packs + 1 ledger create.
- AC-3: `a_thousand_shards_publish_into_shared_packs` (writer, `tests/pack_soak.rs`): 60 packs for 3,000 publishes, 50 a pack.
- AC-4: `a_thousand_shards_publish_into_shared_packs` (writer, `tests/pack_soak.rs`): peak heap 93,830,547 B, bound 128 MiB.

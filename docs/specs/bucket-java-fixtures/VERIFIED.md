# Verified

- AC-1: `the_java_json_manifest_is_read_and_written_back_byte_for_byte`
- AC-2: `every_compact_codec_the_java_wrote_is_read_to_the_same_manifest`, `the_json_and_compact_forms_read_to_one_manifest`
- AC-3: `the_stripe_and_bucket_id_are_the_javas`
- AC-4: `the_ranking_and_the_keep_decision_are_the_javas`, `a_late_joiner_is_held_down_as_long_as_the_java_holds_it`
- AC-5: `the_register_keys_are_the_javas`
- AC-6: NOT-RUN: no Java runs in the gate. On 2026-11-24 the Rust JSON form and its compact forms (none, deflate, zstd, and zstd chosen only when it pays) of the fixture manifest were each read by the Java `BucketManifest.parse` without error, giving bucket `s01-2`, term 9 and members `uuid-a`, `uuid-b`, `uuid-c`.
- Divergence: `a_recorded_length_that_disagrees_with_its_range_is_refused_where_the_java_reads_it`

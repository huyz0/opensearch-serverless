# Bucket manifest reader: verified

Run on 2026-11-20 in a Linux sandbox: `cargo test -p oss-format --test bucket_manifests --test bucket_manifests_corrupt`. No Java process wrote the manifests these tests read: they come from a test-only encoder (`crates/format/tests/bucket_support/mod.rs`) written from the Java's `CompactManifestFormat.body` and `BucketManifest.toBytes`, so a misreading of the Java would be shared by both sides. The first shell-written bucket the R7 suite reads is the differential check (Risks in the spec).

- AC-1: `a_json_manifest_gives_the_named_member_and_its_commit`, `a_json_member_with_no_commit_has_version_zero_and_a_given_version_must_agree`.
- AC-2: `a_compact_commit_parses_to_the_files_ranges_and_lengths_that_were_written`, `a_compact_manifest_gives_the_named_member_with_every_field_the_java_wrote`.
- AC-3: `every_codec_gives_the_member_the_uncompressed_form_does`, `a_large_bucket_reads_through_every_codec_and_lz4_uses_its_matches`.
- AC-4: `an_unknown_uuid_gives_the_bucket_and_term_and_no_member`, `a_compact_member_with_no_commit_has_version_zero`.
- AC-5: `a_compact_commit_parses_to_the_files_ranges_and_lengths_that_were_written`.
- AC-6: `no_proper_prefix_of_a_compact_manifest_is_read_or_panics`, `a_short_header_says_how_many_bytes_it_needs`, `an_unknown_format_or_codec_is_unsupported`, `a_header_length_that_lies_or_is_past_the_cap_is_refused`, `a_string_index_past_the_table_is_refused`, `a_pack_that_is_not_listed_and_unknown_tags_are_refused`, `a_version_and_commit_that_disagree_are_refused`, `an_entry_that_is_not_its_recorded_length_or_has_a_bad_flag_is_refused`, `a_bad_bucket_id_or_string_is_refused`, `a_variable_length_number_past_its_bytes_is_refused`, `a_lz4_stream_with_a_zero_or_far_back_offset_is_refused`, `json_that_is_not_a_manifest_is_refused`.
- AC-7: `the_register_key_is_buckets_id_manifest_under_the_base`.
- AC-8: `a_large_bucket_reads_through_every_codec_and_lz4_uses_its_matches`, `counts_past_the_bytes_left_are_refused_before_anything_is_sized_by_them`.

Mutation sweep (2026-11-20, local, `cargo mutants -p oss-format --file 'crates/format/src/bucket_manifest/*.rs' --in-place`): 216 mutants, 190 caught, 8 unviable, 18 missed. Every miss is equivalent: five replace a `|` of disjoint bit fields with `^` (`cursor.rs` char_unit, `codec.rs` back offset), and thirteen move a bound a count is checked against (`>` to `>=`, `==`, or `/` to `*`) where the read that follows fails on the same input with the same refusal.

# Alias records: verified

Run on 2026-11-16 in a Linux sandbox:
- `cargo test -p oss-format`
- `cargo test -p oss-meta`
- the whole gate, `scripts/gate.sh`

The fixtures were written by the shell's own `AliasRecord`, compiled against the OpenSearch 3.3.0 jars on OpenJDK 21 (`fixtures/aliases/README.md`).

`cargo mutants` over the diff: the four mutants it first missed (a repeated-field bit, two mutation guards, `mutation_pending`) now fail tests.

Each planted defect was seen to fail:

| Planted defect | Test that failed |
|---|---|
| `resolve` that shows an alias still being created | `an_alias_being_created_resolves_as_absent` |
| a write member written whether or not it is a member | `a_built_record_writes_the_javas_bytes` |
| a write member that is not a member kept on parse | `a_write_member_the_java_drops_is_not_kept` |
| a mutation deadline of zero accepted | `every_case_reads_and_writes_as_the_javas`, `a_record_the_java_cannot_read_back_is_not_written` |
| an empty addition accepted | `every_case_reads_and_writes_as_the_javas`, `a_record_the_java_cannot_read_back_is_not_written` |
| no single-index write target | `every_case_reads_and_writes_as_the_javas` |
| a listing that decodes aliases | `an_alias_is_skipped_without_being_decoded` |
| repeated `indices`, `creating` or `additions` accepted | `a_repeated_field_is_refused_as_jackson_refuses_it` |
| a data stream written without its flag | `every_case_reads_and_writes_as_the_javas`, `a_built_record_writes_the_javas_bytes` |

Evidence for each criterion:

- AC-1: `every_case_reads_and_writes_as_the_javas`, `values_the_java_coerces_are_refused`, `a_write_member_the_java_drops_is_not_kept`, `a_repeated_field_is_refused_as_jackson_refuses_it`, `a_repeated_unknown_field_is_kept_where_jackson_refuses_it`
- AC-2: `a_built_record_writes_the_javas_bytes`, `a_record_the_java_cannot_read_back_is_not_written`, `a_name_register_holding_an_alias_reads_as_one`, `an_alias_or_a_tombstone_is_told_apart`
- AC-3: `an_alias_is_created_in_one_request_and_resolves`, `an_alias_and_an_index_cannot_share_a_name`, `an_alias_takes_over_a_deleted_indexs_tombstone`, `an_alias_moves_by_one_swap_from_the_generation_read`, `a_bad_name_or_record_sends_nothing`
- AC-4: `an_alias_being_created_resolves_as_absent`, `an_alias_moves_by_one_swap_from_the_generation_read`, `an_alias_and_an_index_cannot_share_a_name`, `a_data_stream_a_java_node_stored_resolves`, `an_alias_is_not_an_index_and_holds_its_name`, `an_alias_is_skipped_without_being_decoded`

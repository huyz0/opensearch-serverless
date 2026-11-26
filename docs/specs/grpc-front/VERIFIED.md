# Verified

Numbers are from a debug build in a cloud sandbox: provisional, relative only. Slices so far: the listener, framing, Health, limits, deadlines and the status table, and the Bulk mapping. Criteria that need the Search and PIT mapping are NOT-RUN until that slice.

- AC-1: `no_grpc_address_opens_no_port_and_charges_nothing`, `a_grpc_message_limit_of_zero_is_a_start_error`; NOT-RUN: a build without `edge` refusing `OSS_EDGE_GRPC_ADDR` at start is by the same code path as the other listeners and has no test of its own yet
- AC-2: `health_check_tracks_the_node` (clear text, `SERVING`, `NOT_SERVING` on a 503, `NOT_FOUND` for an unknown service); NOT-RUN: the TLS variant
- AC-3: `bulk_is_mapped_to_rest_and_its_answer_back` (wire, stub `/_bulk`), `the_four_actions_become_ndjson` and the other `grpc::bulk` unit tests; NOT-RUN: the differential against REST `_bulk` on a real node and the follow-up Search
- AC-4: NOT-RUN: Search mapping is the next slice
- AC-5: NOT-RUN: PIT mapping is the next slice
- AC-6: Bulk only: `a_field_outside_the_subset_never_reaches_the_node` (`type`, no loopback request), `fields_outside_the_subset_are_refused_by_path`; NOT-RUN: `aggregations`, `knn`, `script_fields` (Search slice) and `extra_field_values`
- AC-7: `an_answer_key_the_table_does_not_know_is_internal`, `grpc::respond` unit tests; NOT-RUN: the property test over the test corpus
- AC-8: `rest_statuses_map_as_the_spec_table`, `messages_are_percent_encoded_and_bounded`, `a_rest_error_keeps_its_body_in_the_status` (429 `circuit_breaking_exception`); NOT-RUN: a loopback answer of each other status through a stub upstream
- AC-9: `big_and_compressed_messages_are_refused_before_buffering` (over-limit is `RESOURCE_EXHAUSTED` from the prefix, a compression flag and `grpc-encoding: gzip` are `UNIMPLEMENTED`, two messages are `INVALID_ARGUMENT`); NOT-RUN: the heap measurement that the refused message was not buffered
- AC-10: `a_deadline_ends_the_call_and_frees_its_slot` (`grpc-timeout: 50m` on a held handler answered `DEADLINE_EXCEEDED` in under 2 s and the next call is served); the slot count itself is not asserted
- AC-11: NOT-RUN: `ServerStreamSearch` is its own slice
- AC-12: `calls_past_max_streams_are_unavailable`
- AC-13: `unknown_methods_and_other_content_types_are_refused`
- AC-14: NOT-RUN: needs the mapping slice, then the shell's client stubs or `grpcurl`
- AC-15: NOT-RUN: needs the mapping slice

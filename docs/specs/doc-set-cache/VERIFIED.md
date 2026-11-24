# Document set cache: verified

Run on 2026-11-24 in a Linux sandbox: `cargo test -p oss-meta`.

- AC-1: `a_snapshot_reads_the_marker_and_only_the_documents_that_moved`, `an_empty_set_is_one_request_and_then_cached`, `a_changed_set_costs_what_changed_not_what_is_held`
- AC-2: `a_replaced_document_is_not_taken_for_the_cached_one`, `a_set_recreated_at_the_same_version_is_not_taken_for_the_cached_one`
- AC-3: `a_set_over_the_budget_is_returned_but_not_kept`, `a_set_just_under_the_budget_is_kept`
- AC-4: `a_document_dropped_mid_read_restarts_from_the_marker`, `a_document_the_store_lost_is_reported`

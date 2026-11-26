# Verified: stock-snapshot-export

- AC-1: `every_recorded_json_round_trips`, `integers_of_every_width_and_strings_of_every_class_round_trip`, `a_double_round_trips_by_value`, `the_bounds_hold_at_their_limit_and_one_past`, `not_json_and_a_big_integer_are_refused_and_the_buffer_keeps_its_start` pass. Jackson 3.2.3 reads the encoder's output for the nine recorded cases it can express to the same tree (`fixtures/snapshots/stock-smile/CheckSmileWrite.java`, by hand, 9 cases, 0 different). The tests compiled red first (the function did not exist), then passed together; no test was seen failing on a wrong result.
- AC-2: NOT-RUN: the header and footer writer is not built yet.
- AC-3: NOT-RUN: the `RepositoryData` writer is not built yet.
- AC-4: NOT-RUN: the metadata and shard snapshot writers are not built yet.
- AC-5: NOT-RUN: the copy and the target store are not built yet.
- AC-6: NOT-RUN: the take and its fencing are not built yet.
- AC-7: NOT-RUN: the take's refusals are not built yet.
- AC-8: NOT-RUN: the export allow-lists and the target store are not built yet.
- AC-9: NOT-RUN: the switch and the route are not built yet.
- AC-10: NOT-RUN: the budget rows wait for the take.
- AC-11: NOT-RUN: needs the take; the stock 3.10 build is at hand (`opensearch-min`, run as a non-root user).

# Concurrent write requests: evidence

- AC-1: `lanes_differ_until_the_counter_wraps`, `sixty_four_generators_drawing_thousands_each_never_collide` (crates/node/tests/id_lanes.rs).
- AC-2: `an_id_has_the_old_layout_and_a_requests_ids_are_in_time_order` (crates/node/tests/id_lanes.rs).
- AC-3: `bulks_to_different_indexes_in_flight_together_share_a_window`, `single_document_writes_in_flight_together_share_a_window` (crates/node/tests/server/boot_write_concurrency.rs). Planted: with the routes back on one shared generator behind a lock both fail (16 requests took 4.05 s, serialised they need 4 s); with the lanes they pass.
- AC-4: `concurrent_bulks_to_one_index_give_every_document_its_own_id` (crates/node/tests/server/boot_write_concurrency.rs).

Throughput before and after (provisional, crates/node/tests/store_usage_measure.rs, a debug build on 4 cores over a file store, bulks of 20 documents each to its own index, a 250 ms window): 1 client 254 ms a bulk both times; 2, 4 and 8 clients took 510, 1,016 and 2,035 ms a bulk before and 254 ms (4 clients, 96 bulks in 6 s against 27) and 508 ms (8 clients, 100 bulks a round against about 30) after.

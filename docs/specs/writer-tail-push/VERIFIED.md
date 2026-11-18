# Verified: the writer pushes acknowledged writes to subscribed readers

- AC-1: `acked_records_go_to_each_subscribed_node_in_one_request_in_order` in crates/node/tests/tail_push.rs
- AC-2: `an_answer_confirms_through_the_end_position_and_a_subscription_starts_at_its_own` in crates/node/tests/tail_push.rs
- AC-3: `answers_out_of_order_confirm_only_through_the_earliest_unanswered` in crates/node/tests/tail_push.rs
- AC-4: `an_unwanted_shard_drops_that_shard_and_a_failed_send_drops_every_shard` in crates/node/tests/tail_push.rs
- AC-5: `a_node_that_fetched_again_keeps_its_new_subscription` in crates/node/tests/tail_push.rs
- AC-6: `a_write_not_to_be_pushed_queues_nothing_and_off_drops_the_shard` in crates/node/tests/tail_push.rs
- AC-7: `past_the_queue_bound_a_shards_subscribers_and_queue_are_dropped` in crates/node/tests/tail_push.rs
- AC-8: `no_subscriber_means_no_send_and_an_empty_queue_sends_nothing` in crates/node/tests/tail_push.rs
- AC-9: `concurrent_acks_and_flushes_deliver_every_record_once_and_confirm_through_the_last` in crates/node/tests/tail_push.rs
- AC-5 (in flight): `an_answer_to_an_old_push_does_not_touch_a_subscription_made_since` in crates/node/tests/tail_push.rs

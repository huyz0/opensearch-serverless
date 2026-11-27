# Verified: the coordinator counts the bytes it exchanges with peers by zone

- AC-1: `same_and_cross_zone_exchanges_count_under_their_paths` in crates/coordinator/tests/it/zone_meter.rs
- AC-2: `a_missing_zone_or_lease_counts_as_unknown` in crates/coordinator/tests/it/zone_meter.rs
- AC-3: `a_failed_call_counts_its_request_only` in crates/coordinator/tests/it/zone_meter.rs
- AC-4: `a_successful_call_returns_its_answer_once` in crates/coordinator/tests/it/zone_meter.rs
- AC-5: `a_fan_out_counts_each_exchange_once` in crates/coordinator/tests/it/zone_meter.rs

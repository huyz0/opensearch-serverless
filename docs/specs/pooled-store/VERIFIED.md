# Pooled blob store: verified

Run with `cargo test -p oss-blob --all-features` and `scripts/gate.sh` on 2026-11-15 (Windows desktop; peak numbers are provisional).

- AC-1: `bodies_round_trip_and_buffers_come_back`, `a_failed_call_still_returns_its_buffer`
- AC-2: `no_free_buffer_is_busy_and_sends_nothing`
- AC-3: `a_16_mib_object_streams_through_one_pooled_buffer` (measured: put 16,777,629 bytes above baseline, the object plus 413; get 0 bytes)

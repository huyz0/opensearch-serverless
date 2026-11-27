# Painless execution limits: verified

Run on 2026-11-27 (Linux cloud container, debug build, emulator store; provisional). The node test was written first and failed (a 2 MiB string was served with 200); the engine's tests (lucene-rust `tests/arena.rs`) were seen failing with the hook and the cap removed. The engine change is huyz0/lucene-rust#616, pinned here at `fc0990e4` (the same change on the base this repo already pinned); #616's CI is red on checks that are red on lucene-rust main too (lucene-zstd, GenMovingFunctions.java), so it is not merged.

- AC-1: `a_script_past_its_limits_is_refused_and_the_node_goes_on` (crates/node/tests/painless_limits.rs): a 2 MiB string answers 400 script_exception, "allocated more than"; a script of 8 KiB is served.
- AC-2: `a_script_past_its_limits_is_refused_and_the_node_goes_on`: 100 strings of 64 KiB add up past 4 MiB and answer 400.
- AC-3: `a_script_past_its_limits_is_refused_and_the_node_goes_on`: 9,000 statements answer 400, "over the limit of 20000".
- AC-4: `a_script_past_its_limits_is_refused_and_the_node_goes_on`: after 20 warm-up failures, 200 more move the process's resident memory from 76,236 KiB to 77,708 KiB (+1.9%), and the next search is 200.
- AC-5: `a_script_past_its_limits_is_refused_and_the_node_goes_on` with the switch off: a Painless script is refused (400) as before.

Not met: growth of a list or map by add and put, and a StringBuilder's appends, are not charged; the 4 MiB is a bound the run is failed at, not bytes reserved from the node's pool; the charge is total allocated, not live bytes.
